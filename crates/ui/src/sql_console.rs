//! Rope-backed SQL console and quote-aware SQL tokenization.
use crate::{data_grid::DataGrid, editor_view::TextEditorView, language::EditorLanguage};
use nozhik_buffer::TextBuffer;
use nozhik_database::{CancellationToken, ConnectionProfile, DatabaseSchemaTree};
use nozhik_language_support::highlighter::HighlightSpan;
use std::ops::Range;

#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent console mode, invalidation, loading and lifecycle flags"
)]
pub struct SqlConsole {
    pub table: Option<crate::table_editor::TableEditor>,
    pub metadata_loading: bool,
    pub retry_metadata: bool,
    pub ddl: bool,
    pub connection_invalid: bool,
    pub path: Option<std::path::PathBuf>,
    pub split: f32,
    pub focus_id: Option<egui::Id>,
    pub editor_id: Option<egui::Id>,
    pub title: String,
    pub connection: String,
    pub schema: String,
    pub buffer: TextBuffer,
    editor: TextEditorView,
    language: EditorLanguage,
    pub results: Vec<DataGrid>,
    pub result_editors: Vec<crate::table_editor::TableEditor>,
    pub executed_sql: String,
    pub result_connection: String,
    pub result_schema: String,
    pub pending_sql: String,
    pub selected_result: usize,
    pub pending: Option<(u64, CancellationToken)>,
    pub error: String,
    highlighted: Option<u64>,
    pub open: bool,
}
impl SqlConsole {
    pub fn new(id: u64, connection: String, schema: String, sql: &str, title: String) -> Self {
        Self {
            table: None,
            metadata_loading: false,
            retry_metadata: false,
            ddl: false,
            connection_invalid: false,
            path: None,
            split: 0.45,
            focus_id: None,
            editor_id: None,
            title,
            connection,
            schema,
            buffer: TextBuffer::load_from_str(nozhik_core::BufferId(id), sql),
            editor: TextEditorView::default(),
            language: EditorLanguage::default(),
            results: Vec::new(),
            result_editors: Vec::new(),
            executed_sql: String::new(),
            result_connection: String::new(),
            result_schema: String::new(),
            pending_sql: String::new(),
            selected_result: 0,
            pending: None,
            error: String::new(),
            highlighted: None,
            open: true,
        }
    }
    pub fn is_dirty(&self) -> bool {
        self.table.as_ref().map_or_else(
            || self.buffer.is_dirty() || self.data_dirty(),
            crate::table_editor::TableEditor::dirty,
        )
    }
    /// Returns a request only after toolbar/shortcut execution.
    #[allow(
        clippy::too_many_lines,
        reason = "SQL toolbar, Rope editor and result set tabs form one console view"
    )]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        profiles: &[ConnectionProfile],
        tree: Option<&DatabaseSchemaTree>,
        enabled: bool,
    ) -> Option<String> {
        if self.metadata_loading || self.table.as_ref().is_some_and(|t| !t.info.details_loaded) {
            self.focus_id = None;
            self.editor_id = None;
            ui.centered_and_justified(|ui| {
                ui.vertical_centered(|ui| {
                    if self.metadata_loading && !self.connection_invalid {
                        ui.spinner();
                        ui.label("Loading table metadata…");
                    } else if ui
                        .add_enabled(enabled, egui::Button::new("Retry loading table"))
                        .clicked()
                    {
                        self.retry_metadata = true;
                    }
                });
            });
            return None;
        }
        if let Some(table) = &mut self.table {
            self.focus_id = None;
            self.editor_id = None;
            if table.changes.is_none()
                && let Some((_, cancel)) = &self.pending
            {
                ui.centered_and_justified(|ui| {
                    ui.vertical_centered(|ui| {
                        ui.spinner();
                        ui.label("Loading table data…");
                        if ui.button("Cancel loading").clicked() {
                            cancel.cancel();
                        }
                    });
                });
                return None;
            }
            table.show(ui, self.pending.is_some(), enabled);
            return None;
        }
        let results_dirty = self
            .result_editors
            .iter()
            .any(crate::table_editor::TableEditor::dirty);
        let result_modal = self
            .result_editors
            .iter()
            .any(crate::table_editor::TableEditor::modal_open);
        let mut execute = false;
        ui.add_enabled_ui(
            enabled && !results_dirty && !result_modal && self.pending.is_none(),
            |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_salt("sql_connection")
                        .selected_text(&self.connection)
                        .show_ui(ui, |ui| {
                            for p in profiles {
                                if ui
                                    .selectable_value(&mut self.connection, p.name.clone(), &p.name)
                                    .changed()
                                {
                                    self.schema.clear();
                                }
                            }
                        });
                    egui::ComboBox::from_id_salt("sql_schema")
                        .selected_text(if self.schema.is_empty() {
                            "Default schema"
                        } else {
                            &self.schema
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut self.schema, String::new(), "Default schema");
                            if let Some(tree) = tree {
                                for s in &tree.schemas {
                                    ui.selectable_value(&mut self.schema, s.name.clone(), &s.name);
                                }
                            }
                        });
                    execute = ui
                        .add_enabled(
                            self.pending.is_none() && !self.ddl,
                            egui::Button::new(
                                egui::RichText::new("▶ Execute").color(crate::theme::SUCCESS),
                            ),
                        )
                        .clicked();
                });
            },
        );
        if let Some((_, cancel)) = &self.pending {
            ui.horizontal(|ui| {
                ui.spinner();
                if ui.button("Cancel").clicked() {
                    cancel.cancel();
                }
            });
        }
        if enabled
            && self.pending.is_none()
            && !results_dirty
            && !result_modal
            && !self.ddl
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
        {
            execute = true;
        }
        self.language.sql_catalog = tree
            .zip(profiles.iter().find(|p| p.name == self.connection))
            .map(|(tree, p)| (tree.clone(), self.schema.clone(), p.driver));
        self.language.triggers.member_access_triggers = vec![".".into()];
        self.language.triggers.auto_popup_triggers = vec!['.'];
        let revision = self.buffer.snapshot().revision;
        if self.highlighted != Some(revision) {
            let source = self.buffer.rope().to_string();
            self.language.spans = tokens(&source)
                .into_iter()
                .filter_map(|(range, kind)| {
                    kind.map(|token| HighlightSpan {
                        range,
                        token: token.into(),
                    })
                })
                .collect();
            self.language.revision = revision;
            self.highlighted = Some(revision);
        }
        let total_height = ui.available_height();
        let editor_height = if self.results.is_empty() {
            ui.available_height()
        } else {
            (ui.available_height() * self.split).max(70.0)
        };
        ui.allocate_ui(egui::vec2(ui.available_width(), editor_height), |ui| {
            let id = ui.id().with(("editor", self.buffer.snapshot().buffer_id));
            self.editor_id = Some(id);
            self.focus_id = ui.memory(|m| m.has_focus(id)).then_some(id);
            if let Err(e) = self.editor.show_language(
                ui,
                &mut self.buffer,
                14.0,
                4,
                enabled,
                Some(&mut self.language),
            ) {
                self.error = e.to_string();
            }
        });
        if !self.results.is_empty() {
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), crate::theme::SPLITTER),
                egui::Sense::drag(),
            );
            ui.painter().rect_filled(rect, 0.0, crate::theme::BORDER);
            response
                .clone()
                .on_hover_cursor(egui::CursorIcon::ResizeVertical)
                .on_hover_text("Resize SQL editor / results");
            if response.dragged() {
                self.split = (self.split + response.drag_delta().y / total_height).clamp(0.15, 0.8);
            }
            ui.horizontal(|ui| {
                for (index, _) in self.results.iter().enumerate() {
                    ui.selectable_value(
                        &mut self.selected_result,
                        index,
                        format!("Result {}", index + 1),
                    );
                }
            });
            let valid =
                self.connection == self.result_connection && self.schema == self.result_schema;
            if let Some(editor) = self.result_editors.get_mut(self.selected_result) {
                ui.push_id(("sql_result", self.selected_result), |ui| {
                    editor.show(ui, self.pending.is_some(), enabled && valid);
                });
            } else if let Some(grid) = self.results.get_mut(self.selected_result) {
                grid.show(ui);
            }
        }
        if execute {
            Some(self.execution_sql())
        } else {
            None
        }
    }
    pub(crate) fn result_context_valid(&self) -> bool {
        self.connection == self.result_connection && self.schema == self.result_schema
    }
    pub(crate) fn data_dirty(&self) -> bool {
        self.table
            .as_ref()
            .is_some_and(|t| t.dirty() || self.pending.is_some())
            || self
                .result_editors
                .iter()
                .any(crate::table_editor::TableEditor::dirty)
            || (!self.result_editors.is_empty() && self.pending.is_some())
    }
    pub(crate) fn active_table(&self) -> Option<&crate::table_editor::TableEditor> {
        self.table
            .as_ref()
            .or_else(|| self.result_editors.get(self.selected_result))
    }
    pub(crate) fn active_table_mut(&mut self) -> Option<&mut crate::table_editor::TableEditor> {
        self.table
            .as_mut()
            .or_else(|| self.result_editors.get_mut(self.selected_result))
    }
    pub(crate) fn loaded_results(
        &mut self,
        results: Vec<nozhik_database::QueryResult>,
        tree: Option<&DatabaseSchemaTree>,
        driver: nozhik_database::DbDriverType,
    ) {
        self.executed_sql.clone_from(&self.pending_sql);
        self.result_connection.clone_from(&self.connection);
        self.result_schema.clone_from(&self.schema);
        self.results = results.into_iter().map(DataGrid::new).collect();
        self.rebuild_result_editors(tree, driver);
    }
    pub(crate) fn rebuild_result_editors(
        &mut self,
        tree: Option<&DatabaseSchemaTree>,
        driver: nozhik_database::DbDriverType,
    ) {
        self.result_editors = self
            .results
            .iter()
            .map(|grid| &grid.result)
            .map(|result| {
                let source = tree.and_then(|tree| {
                    crate::sql_support::select_source(
                        &self.executed_sql,
                        &self.result_schema,
                        tree,
                        result,
                    )
                });
                let info = source
                    .as_ref()
                    .and_then(|source| {
                        tree?
                            .schemas
                            .iter()
                            .find(|s| s.name == source.schema)?
                            .tables
                            .iter()
                            .find(|t| t.name == source.table)
                    })
                    .cloned()
                    .unwrap_or_else(|| nozhik_database::introspection::TableInfo {
                        name: "Result".into(),
                        is_view: true,
                        columns: Vec::new(),
                        objects: nozhik_database::catalog::TableObjects::default(),
                        details_loaded: true,
                    });
                let mut editor = crate::table_editor::TableEditor::new(driver, info);
                editor.console_result = true;
                editor.loaded(
                    source
                        .as_ref()
                        .map_or(self.result_schema.as_str(), |s| s.schema.as_str()),
                    result.clone(),
                );
                if let Some(changes) = &mut editor.changes {
                    changes.insert_defaults = source.map_or_else(Vec::new, |s| s.defaults);
                }
                editor
            })
            .collect();
    }
    pub(crate) const fn reveal(&mut self, line: Option<usize>) {
        self.editor.reveal_line = line;
    }
    pub fn execution_sql(&self) -> String {
        let source = self.buffer.rope().to_string();
        let Some(cursor) = self.buffer.cursors().as_slice().first() else {
            return source;
        };
        let start = cursor.anchor.byte_offset.min(cursor.head.byte_offset);
        let end = cursor.anchor.byte_offset.max(cursor.head.byte_offset);
        if start != end {
            return source.get(start..end).unwrap_or("").into();
        }
        statement_at(&source, cursor.head.byte_offset).into()
    }
}
impl Drop for SqlConsole {
    fn drop(&mut self) {
        if let Some((_, token)) = &self.pending {
            token.cancel();
        }
    }
}
pub fn statement_at(source: &str, caret: usize) -> &str {
    let mut start = 0;
    for (range, kind) in tokens(source) {
        if kind.is_none() && source.get(range.clone()) == Some(";") {
            if caret <= range.end {
                return source.get(start..range.end).unwrap_or(source).trim();
            }
            start = range.end;
        }
    }
    let tail = source.get(start..).unwrap_or(source).trim();
    if tail.is_empty() { source.trim() } else { tail }
}
/// Lexes strings, quoted identifiers, dollar quotes, comments and SQL words.
pub fn tokens(source: &str) -> Vec<(Range<usize>, Option<&'static str>)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let mut kind = None;
        if bytes.get(i..i + 2) == Some(b"--") || bytes.get(i) == Some(&b'#') {
            i += 1;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            kind = Some("comment");
        } else if bytes.get(i..i + 2) == Some(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes.get(i..i + 2) == Some(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes.get(i..i + 2) == Some(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            kind = Some("comment");
        } else if matches!(bytes[i], b'\'' | b'"' | b'`' | b'[') {
            let quote = if bytes[i] == b'[' { b']' } else { bytes[i] };
            i += 1;
            while i < bytes.len() {
                if bytes[i] == quote {
                    i += 1;
                    if bytes.get(i) == Some(&quote) {
                        i += 1;
                    } else {
                        break;
                    }
                } else if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else {
                    i += 1;
                }
            }
            kind = Some("string");
        } else if bytes[i] == b'$' && dollar_delimiter(&source[i..]).is_some() {
            let delimiter = dollar_delimiter(&source[i..]).unwrap_or("$");
            i += delimiter.len();
            i = source[i..]
                .find(delimiter)
                .map_or(bytes.len(), |end| i + end + delimiter.len());
            kind = Some("string");
        } else if source[i..]
            .chars()
            .next()
            .is_some_and(|c| c.is_alphabetic() || c == '_')
        {
            while let Some(c) = source[i..]
                .chars()
                .next()
                .filter(|c| c.is_alphanumeric() || matches!(c, '_' | '$'))
            {
                i += c.len_utf8();
            }
            let word = source[start..i].to_ascii_uppercase();
            if "SELECT FROM WHERE AS AND OR NOT NULL TRUE FALSE JOIN INNER LEFT RIGHT FULL ON GROUP BY ORDER ASC DESC LIMIT OFFSET INSERT INTO VALUES UPDATE SET DELETE CREATE TABLE VIEW INDEX DROP ALTER PRIMARY KEY REFERENCES DEFAULT DISTINCT UNION ALL HAVING WITH RECURSIVE RETURNING BEGIN COMMIT ROLLBACK EXPLAIN CASE WHEN THEN ELSE END IS IN LIKE EXISTS USE".split_whitespace().any(|k| k==word) {kind=Some("keyword");}
        } else if bytes[i].is_ascii_digit() {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            kind = Some("number");
        } else {
            i += source[i..].chars().next().map_or(1, char::len_utf8);
        }
        // Quoted byte scanning always terminates on ASCII or EOF (UTF-8 boundary).
        while !source.is_char_boundary(i) {
            i += 1;
        }
        out.push((start..i, kind));
    }
    out
}
fn dollar_delimiter(source: &str) -> Option<&str> {
    let tail = source.strip_prefix('$')?;
    let end = tail.find('$')?;
    if tail[..end]
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_')
    {
        source.get(..end + 2)
    } else {
        None
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn statement_selection_ignores_quoted_and_commented_semicolons() {
        let sql = "SELECT 'a;b'; -- ;\nSELECT $$x;y$$, \"semi;colon\"; SELECT 3";
        assert_eq!(statement_at(sql, 5), "SELECT 'a;b';");
        assert_eq!(
            statement_at(sql, 30),
            "-- ;\nSELECT $$x;y$$, \"semi;colon\";"
        );
        assert_eq!(statement_at(sql, sql.len()), "SELECT 3");
        assert!(
            tokens("SELECT 'Привет'; /* nested /* ; */ */")
                .iter()
                .any(|(_, k)| *k == Some("comment"))
        );
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    #[test]
    fn selected_sql_wins_over_statement_at_caret() -> Result<(), Box<dyn std::error::Error>> {
        let mut console = SqlConsole::new(
            77,
            "connection".into(),
            String::new(),
            "SELECT 1; SELECT 2;",
            "test".into(),
        );
        let anchor = console.buffer.position_at(10)?;
        let head = console.buffer.position_at(18)?;
        console
            .buffer
            .set_cursors(vec![nozhik_buffer::Cursor { anchor, head }])?;
        assert_eq!(console.execution_sql(), "SELECT 2");
        Ok(())
    }
}

#[cfg(test)]
mod completion_tests {
    use super::*;
    use egui::{Event, Key, Modifiers};
    fn frame(console: &mut SqlConsole, ctx: &egui::Context, events: Vec<Event>) {
        let (tree, _) = crate::sql_support::tests::fixture();
        let profile = ConnectionProfile {
            name: "db".into(),
            ..Default::default()
        };
        let _output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    console.show(ui, std::slice::from_ref(&profile), Some(&tree), true);
                });
            },
        );
    }
    #[test]
    fn keyboard_sql_completion_uses_schema_popup_and_replaces_prefix()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut console = SqlConsole::new(
            800,
            "db".into(),
            "main".into(),
            "SELECT * FROM ",
            "console".into(),
        );
        let ctx = egui::Context::default();
        let pos = console
            .buffer
            .position_at(console.buffer.rope().len_bytes())?;
        console.buffer.set_cursors(vec![nozhik_buffer::Cursor {
            anchor: pos,
            head: pos,
        }])?;
        frame(&mut console, &ctx, vec![]);
        ctx.memory_mut(|m| m.request_focus(console.editor_id.unwrap_or(egui::Id::NULL)));
        frame(&mut console, &ctx, vec![Event::Text("go".into())]);
        assert!(
            console
                .language
                .completions
                .iter()
                .any(|i| i.label == "goods")
        );
        frame(
            &mut console,
            &ctx,
            vec![Event::Key {
                key: Key::Tab,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert_eq!(console.buffer.rope().to_string(), "SELECT * FROM \"goods\"");
        frame(&mut console, &ctx, vec![Event::Text(" WHERE ty".into())]);
        assert!(
            console
                .language
                .completions
                .iter()
                .any(|i| i.label == "type")
        );
        frame(
            &mut console,
            &ctx,
            vec![Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            }],
        );
        assert_eq!(
            console.buffer.rope().to_string(),
            "SELECT * FROM \"goods\" WHERE \"type\""
        );
        frame(&mut console, &ctx, vec![Event::Text("='ty".into())]);
        assert_eq!(console.language.completions, []);
        Ok(())
    }
}
