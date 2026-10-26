//! Rope-backed SQL console and quote-aware SQL tokenization.
use crate::{data_grid::DataGrid, editor_view::TextEditorView, language::EditorLanguage};
use nozhik_buffer::TextBuffer;
use nozhik_database::{CancellationToken, ConnectionProfile, DatabaseSchemaTree};
use nozhik_language_support::highlighter::HighlightSpan;
use std::ops::Range;

pub struct SqlConsole {
    pub title: String,
    pub connection: String,
    pub schema: String,
    pub buffer: TextBuffer,
    editor: TextEditorView,
    language: EditorLanguage,
    pub results: Vec<DataGrid>,
    pub selected_result: usize,
    pub pending: Option<(u64, CancellationToken)>,
    pub error: String,
    highlighted: Option<u64>,
    pub open: bool,
}
impl SqlConsole {
    pub fn new(id: u64, connection: String, schema: String, sql: &str, title: String) -> Self {
        Self {
            title,
            connection,
            schema,
            buffer: TextBuffer::load_from_str(nozhik_core::BufferId(id), sql),
            editor: TextEditorView::default(),
            language: EditorLanguage::default(),
            results: Vec::new(),
            selected_result: 0,
            pending: None,
            error: String::new(),
            highlighted: None,
            open: true,
        }
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
        let mut execute = false;
        ui.add_enabled_ui(enabled, |ui| {
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
                        self.pending.is_none(),
                        egui::Button::new(
                            egui::RichText::new("▶ Execute").color(egui::Color32::LIGHT_GREEN),
                        ),
                    )
                    .clicked();
                if let Some((_, cancel)) = &self.pending {
                    ui.spinner();
                    if ui.button("Cancel").clicked() {
                        cancel.cancel();
                    }
                }
            });
        });
        if enabled
            && self.pending.is_none()
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter))
        {
            execute = true;
        }
        if !self.error.is_empty() {
            ui.colored_label(egui::Color32::LIGHT_RED, &self.error);
        }
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
        let editor_height = if self.results.is_empty() {
            ui.available_height()
        } else {
            (ui.available_height() * 0.45).max(70.0)
        };
        ui.allocate_ui(egui::vec2(ui.available_width(), editor_height), |ui| {
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
            ui.separator();
            ui.horizontal(|ui| {
                for (index, _) in self.results.iter().enumerate() {
                    ui.selectable_value(
                        &mut self.selected_result,
                        index,
                        format!("Result {}", index + 1),
                    );
                }
            });
            if let Some(grid) = self.results.get_mut(self.selected_result) {
                grid.show(ui);
            }
        }
        if execute {
            Some(self.execution_sql())
        } else {
            None
        }
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
fn statement_at(source: &str, caret: usize) -> &str {
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
fn tokens(source: &str) -> Vec<(Range<usize>, Option<&'static str>)> {
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
        } else if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
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
