//! Table data editing with local staging and explicit submission.
use nozhik_database::table_query::TableQuery;
use nozhik_database::{
    DbDriverType, DbValue, QueryResult, changes::TableChanges, introspection::TableInfo,
};
use std::collections::{BTreeSet, HashMap};

#[derive(Clone, Copy)]
pub enum TableAction {
    Reload,
    Query,
    Submit,
    Ddl,
    Cancel,
    Page { offset: usize, size: usize },
}
pub struct TableEditor {
    pub driver: DbDriverType,
    pub console_result: bool,
    pub info: TableInfo,
    pub changes: Option<TableChanges>,
    pub action: Option<TableAction>,
    pub where_input: String,
    pub order_input: String,
    pub query: TableQuery,
    pending_query: Option<TableQuery>,
    offset: usize,
    page_size: usize,
    has_more: bool,
    pub pending_page: Option<(usize, usize)>,
    selected: BTreeSet<usize>,
    preview: bool,
    inputs: HashMap<(usize, usize), String>,
    reveal_row: Option<usize>,
}
impl TableEditor {
    pub fn new(driver: DbDriverType, info: TableInfo) -> Self {
        Self {
            driver,
            console_result: false,
            info,
            changes: None,
            action: None,
            where_input: String::new(),
            order_input: String::new(),
            query: TableQuery::default(),
            pending_query: None,
            offset: 0,
            page_size: 500,
            has_more: false,
            pending_page: None,
            selected: BTreeSet::new(),
            preview: false,
            inputs: HashMap::new(),
            reveal_row: None,
        }
    }
    pub fn loaded(&mut self, schema: &str, mut result: QueryResult) {
        if let Some(query) = self.pending_query.take() {
            self.query = query;
        }
        if let Some((offset, size)) = self.pending_page.take() {
            self.offset = offset;
            self.page_size = size;
        }
        self.has_more = !self.console_result && result.rows.len() > self.page_size;
        if !self.console_result {
            result.rows.truncate(self.page_size);
        }
        self.reveal_row = Some(0);
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
    pub fn page_sql(&self, schema: &str) -> nozhik_database::Result<String> {
        let (offset, size) = self.pending_page.unwrap_or((self.offset, self.page_size));
        self.pending_query.as_ref().unwrap_or(&self.query).page_sql(
            self.driver,
            schema,
            &self.info,
            offset,
            size,
        )
    }
    pub fn request_query(&mut self) {
        self.pending_query = Some(TableQuery {
            predicate: self.where_input.clone(),
            order: self.order_input.clone(),
        });
        self.pending_page = Some((0, self.page_size));
    }
    pub fn query_failed(&mut self) {
        self.pending_page = None;
        self.pending_query = None;
    }
    fn cycle_sort(&mut self, column: usize) {
        if let Some(column) = self
            .changes
            .as_ref()
            .and_then(|c| c.baseline.columns.get(column))
            .cloned()
        {
            let quoted = nozhik_database::connection::quote_identifier(self.driver, &column.name);
            self.order_input = match self.query.column_descending(self.driver, &column.name) {
                None => format!("{quoted} ASC"),
                Some(false) => format!("{quoted} DESC"),
                Some(true) => String::new(),
            };
            if self.console_result {
                self.query.order.clone_from(&self.order_input);
                if let Some(changes) = &mut self.changes {
                    changes.revert();
                    if let Some(descending) =
                        self.query.column_descending(self.driver, &column.name)
                    {
                        let index = changes
                            .baseline
                            .columns
                            .iter()
                            .position(|c| c.name == column.name)
                            .unwrap_or(0);
                        changes.rows.sort_by(|a, b| {
                            let ordering = a
                                .values
                                .get(index)
                                .and_then(Option::as_ref)
                                .zip(b.values.get(index).and_then(Option::as_ref))
                                .map_or(std::cmp::Ordering::Equal, |(a, b)| {
                                    crate::data_grid::compare(a, b)
                                });
                            if descending {
                                ordering.reverse()
                            } else {
                                ordering
                            }
                        });
                    }
                }
                self.inputs.clear();
                self.selected.clear();
            } else {
                self.action = Some(TableAction::Query);
            }
        }
    }
    fn query_fields(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let mut apply = false;
        egui::Frame::new()
            .fill(crate::theme::PANEL)
            .stroke(egui::Stroke::new(1.0_f32, crate::theme::BORDER))
            .inner_margin(4.0)
            .show(ui, |ui| {
                ui.add_enabled_ui(enabled, |ui| {
                    let width = ((ui.available_width() - 95.0) * 0.5).max(80.0);
                    ui.horizontal(|ui| {
                        let (enter, clear) = query_field(ui, "WHERE", &mut self.where_input, width);
                        apply |= enter || clear;
                        ui.separator();
                        let (enter, clear) =
                            query_field(ui, "ORDER BY", &mut self.order_input, width);
                        apply |= enter || clear;
                        apply |= ui
                            .button("Apply")
                            .on_hover_text("Apply WHERE and ORDER BY (Enter)")
                            .clicked();
                    });
                });
            });
        if apply {
            self.action = Some(TableAction::Query);
        }
    }
    fn pagination(&mut self, ui: &mut egui::Ui, enabled: bool) {
        let count = self.changes.as_ref().map_or(0, |c| c.baseline.rows.len());
        let truncated = self.changes.as_ref().is_some_and(|c| c.baseline.truncated);
        let end = self.offset.saturating_add(count);
        let start = if count == 0 { 0 } else { self.offset + 1 };
        ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
            egui::Frame::new()
                .fill(crate::theme::PANEL)
                .stroke(egui::Stroke::new(1.0_f32, crate::theme::BORDER))
                .corner_radius(8.0)
                .inner_margin(8.0)
                .show(ui, |ui| {
                    ui.add_enabled_ui(enabled && self.changes.is_some(), |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(self.offset > 0, egui::Button::new("|<"))
                                .on_hover_text("First page")
                                .clicked()
                            {
                                self.action = Some(TableAction::Page {
                                    offset: 0,
                                    size: self.page_size,
                                });
                            }
                            if ui
                                .add_enabled(self.offset > 0, egui::Button::new("<"))
                                .on_hover_text("Previous page")
                                .clicked()
                            {
                                self.action = Some(TableAction::Page {
                                    offset: self.offset.saturating_sub(self.page_size),
                                    size: self.page_size,
                                });
                            }
                            ui.label(format!("{start}–{end}"));
                            let mut size = self.page_size;
                            egui::ComboBox::from_id_salt("table_page_size")
                                .width(65.0)
                                .selected_text(format!("{} rows", self.page_size))
                                .show_ui(ui, |ui| {
                                    for option in [100, 500, 1000, 5000] {
                                        ui.selectable_value(
                                            &mut size,
                                            option,
                                            format!("{option} rows"),
                                        );
                                    }
                                });
                            if size != self.page_size {
                                self.action = Some(TableAction::Page { offset: 0, size });
                            }
                            ui.label(if truncated {
                                "of ? · size limit".into()
                            } else if self.has_more {
                                format!("of {}+", end.saturating_add(1))
                            } else {
                                format!("of {end}")
                            });
                            if ui
                                .add_enabled(self.has_more, egui::Button::new(">"))
                                .on_hover_text("Next page")
                                .clicked()
                            {
                                self.action = Some(TableAction::Page {
                                    offset: self.offset.saturating_add(self.page_size),
                                    size: self.page_size,
                                });
                            }
                        });
                    })
                    .response
                    .on_disabled_hover_text(
                        "Finish loading or Submit / Revert pending changes to change pages",
                    );
                });
        });
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
        if !self.console_result {
            self.query_fields(ui, enabled && !busy && !self.preview && !self.dirty());
        } else if self.info.is_view {
            ui.label("Read-only result: editing requires direct columns from one base table with its complete primary key.");
        }
        let mut sort_column = None;
        if let Some(changes) = &mut self.changes {
            let size = egui::vec2(
                ui.available_width(),
                (ui.available_height() - 46.0).max(60.0),
            );
            ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.add_enabled_ui(enabled && !busy && !self.preview, |ui| {
                    sort_column = self::grid(
                        ui,
                        changes,
                        &mut self.selected,
                        &mut self.inputs,
                        self.reveal_row.take(),
                        GridOptions {
                            editable: !self.info.is_view,
                            offset: self.offset,
                            driver: self.driver,
                            query: &self.query,
                            can_sort: !dirty,
                        },
                    );
                });
            });
        }
        if let Some(column) = sort_column {
            self.cycle_sort(column);
        }
        if self.console_result {
            if let Some(changes) = &self.changes {
                ui.label(format!(
                    "{} rows{} · {} ms · {} affected",
                    changes.baseline.rows.len(),
                    if changes.baseline.truncated {
                        " · result limit"
                    } else {
                        ""
                    },
                    changes.baseline.execution_time_ms,
                    changes.baseline.rows_affected
                ));
            }
        } else {
            self.pagination(ui, enabled && !busy && !self.preview && !self.dirty());
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
    options: GridOptions<'_>,
) -> Option<usize> {
    let GridOptions {
        editable,
        offset,
        driver,
        query,
        can_sort,
    } = options;
    let mut sort_column = None;
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
                            offset + i + 1,
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
                    // Preserve spatial values losslessly while editing ordinary columns.
                    let can_edit = editable
                        && !row.deleted
                        && !changes.baseline.columns[c]
                            .data_type
                            .eq_ignore_ascii_case("GEOMETRY");
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
                let arrow = egui::Rect::from_min_size(
                    cell.right_top() - egui::vec2(26.0, 0.0),
                    egui::vec2(26.0, header),
                );
                let direction = query.column_descending(driver, &column.name);
                if sort_arrow(
                    ui,
                    arrow,
                    ui.id().with(("table_sort", c)),
                    direction,
                    can_sort,
                ) {
                    sort_column = Some(c);
                }
                let label_rect = egui::Rect::from_min_max(cell.min, arrow.left_bottom());
                ui.painter()
                    .with_clip_rect(label_rect.intersect(ui.clip_rect()))
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
    sort_column
}

#[derive(Clone, Copy)]
struct GridOptions<'a> {
    editable: bool,
    offset: usize,
    driver: DbDriverType,
    query: &'a TableQuery,
    can_sort: bool,
}

fn clause_icon(ui: &mut egui::Ui, filter: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(17.0, 20.0), egui::Sense::hover());
    let stroke = egui::Stroke::new(1.3_f32, crate::theme::TEXT);
    if filter {
        let points = [
            (1.0, 3.0),
            (15.0, 3.0),
            (10.0, 9.0),
            (10.0, 15.0),
            (6.0, 18.0),
            (6.0, 9.0),
            (1.0, 3.0),
        ]
        .into_iter()
        .map(|(x, y)| rect.min + egui::vec2(x, y))
        .collect();
        ui.painter().add(egui::Shape::line(points, stroke));
    } else {
        for (y, end) in [(5.0, 15.0), (10.0, 11.0), (15.0, 7.0)] {
            ui.painter().line_segment(
                [rect.min + egui::vec2(2.0, y), rect.min + egui::vec2(end, y)],
                stroke,
            );
        }
    }
}

fn query_field(ui: &mut egui::Ui, label: &str, value: &mut String, width: f32) -> (bool, bool) {
    let mut enter = false;
    let mut clear = false;
    ui.allocate_ui_with_layout(
        egui::vec2(width, 27.0),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            clause_icon(ui, label == "WHERE");
            ui.label(
                egui::RichText::new(label)
                    .monospace()
                    .color(if value.is_empty() {
                        crate::theme::MUTED
                    } else {
                        egui::Color32::from_rgb(204, 120, 88)
                    }),
            );
            let edit_width = (ui.available_width() - 24.0).max(20.0);
            let response = ui.add_sized(
                [edit_width, 25.0],
                egui::TextEdit::singleline(value)
                    .id_salt(("table_clause", label))
                    .font(egui::TextStyle::Monospace)
                    .hint_text("expression")
                    .frame(false),
            );
            enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui
                .add_enabled(!value.is_empty(), egui::Button::new("×").frame(false))
                .on_hover_text(format!("Clear {label}"))
                .clicked()
            {
                value.clear();
                clear = true;
            }
        },
    );
    (enter, clear)
}

fn sort_arrow(
    ui: &egui::Ui,
    rect: egui::Rect,
    id: egui::Id,
    descending: Option<bool>,
    enabled: bool,
) -> bool {
    let response = ui.interact(
        rect,
        id,
        if enabled {
            egui::Sense::click()
        } else {
            egui::Sense::hover()
        },
    );
    let color = if !enabled || !ui.is_enabled() {
        crate::theme::MUTED
    } else if descending.is_some() || response.hovered() {
        crate::theme::TEXT
    } else {
        crate::theme::MUTED
    };
    let center = rect.center();
    for down in [false, true] {
        if descending.is_some_and(|direction| direction != down) {
            continue;
        }
        let y = if descending.is_none() {
            if down { 4.0 } else { -4.0 }
        } else {
            0.0
        };
        let sign: f32 = if down { 1.0 } else { -1.0 };
        let points = vec![
            center + egui::vec2(-3.5, sign.mul_add(-2.0, y)),
            center + egui::vec2(3.5, sign.mul_add(-2.0, y)),
            center + egui::vec2(0.0, sign.mul_add(2.0, y)),
        ];
        ui.painter()
            .with_clip_rect(rect.intersect(ui.clip_rect()))
            .add(egui::Shape::convex_polygon(
                points,
                color,
                egui::Stroke::NONE,
            ));
    }
    response
        .on_hover_text("Sort: None → ASC → DESC → None")
        .clicked()
        && enabled
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

#[cfg(test)]
mod console_tests {
    use super::*;
    #[test]
    fn console_sort_cycles_without_changing_row_identity_or_insert_defaults()
    -> Result<(), Box<dyn std::error::Error>> {
        let (tree, mut result) = crate::sql_support::tests::fixture();
        result.rows = vec![
            vec![
                DbValue::Int(10),
                DbValue::Int(11),
                DbValue::Text("last".into()),
            ],
            vec![
                DbValue::Int(2),
                DbValue::Int(11),
                DbValue::Text("first".into()),
            ],
        ];
        let mut editor = TableEditor::new(DbDriverType::Sqlite, tree.schemas[0].tables[0].clone());
        editor.console_result = true;
        editor.loaded("main", result);
        editor.changes.as_mut().ok_or("changes")?.insert_defaults =
            vec![("type".into(), DbValue::Int(11))];
        for (expected, direction) in [(2, Some(false)), (10, Some(true)), (10, None)] {
            editor.cycle_sort(0);
            assert_eq!(
                editor.query.column_descending(editor.driver, "id"),
                direction
            );
            let changes = editor.changes.as_ref().ok_or("changes")?;
            assert_eq!(
                changes.rows[0].original.as_ref().ok_or("original")?[0],
                DbValue::Int(expected)
            );
            assert!(!editor.dirty());
            assert!(editor.action.is_none());
            assert_eq!(
                changes.insert_defaults,
                vec![("type".into(), DbValue::Int(11))]
            );
        }
        Ok(())
    }
}
