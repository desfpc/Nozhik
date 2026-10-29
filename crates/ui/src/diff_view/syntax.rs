//! Immutable old/new PHP snapshots are parsed off the UI thread, without LSP.
use crate::theme;
use egui::{FontId, text::LayoutJob};
use nozhik_language_support::highlighter::{HighlightSpan, Highlighter};
use nozhik_vcs::model::{DiffLineKind, FileDiff};
use std::sync::mpsc;

type Lines = Vec<Vec<Vec<HighlightSpan>>>;

pub(super) struct Syntax {
    enabled: bool,
    palette: String,
    started: bool,
    receiver: Option<mpsc::Receiver<Lines>>,
    lines: Lines,
}
impl Default for Syntax {
    fn default() -> Self {
        Self {
            enabled: true,
            palette: String::new(),
            started: false,
            receiver: None,
            lines: Vec::new(),
        }
    }
}
impl Syntax {
    pub(super) fn configure(&mut self, config: &nozhik_config::Config, file: Option<&FileDiff>) {
        let Some(file) = file else {
            return;
        };
        let enabled =
            nozhik_config::languages::enabled_for_path(&config.enabled_languages, &file.path);
        if self.enabled != enabled {
            self.started = false;
            self.receiver = None;
            self.lines.clear();
            self.enabled = enabled;
        }
        let extension = file
            .path
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();
        self.palette = config
            .file_types
            .get(&extension)
            .map_or_else(String::new, |s| s.syntax_theme.clone());
    }
    pub(super) fn palette(&self) -> &str {
        &self.palette
    }
    pub(super) fn spans(&self, h: usize, l: usize) -> &[HighlightSpan] {
        self.lines
            .get(h)
            .and_then(|lines| lines.get(l))
            .map_or(&[], Vec::as_slice)
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context, file: Option<&FileDiff>) {
        if let Some(receiver) = &self.receiver
            && let Ok(lines) = receiver.try_recv()
        {
            self.lines = lines;
            self.receiver = None;
        }
        let Some(file) = file else {
            return;
        };
        if self.started
            || !self.enabled
            || self.palette == "plain"
            || file.binary
            || file.truncated
            || nozhik_config::languages::language_for_path(&file.path)
                != Some(nozhik_config::languages::LanguageId::Php)
        {
            return;
        }
        self.started = true;
        let file = file.clone();
        let ctx = ctx.clone();
        let (sender, receiver) = mpsc::channel();
        self.receiver = Some(receiver);
        std::thread::spawn(move || {
            if let Ok(lines) = highlight(&file) {
                let _delivered = sender.send(lines);
                ctx.request_repaint();
            }
        });
    }
}
fn highlight(file: &FileDiff) -> nozhik_language_support::Result<Lines> {
    // Partial hunks lack lexical context (e.g. multiline comments); keep them plain.
    if file.hunks.len() != 1 || file.hunks[0].old_start > 1 || file.hunks[0].new_start > 1 {
        return Ok(Vec::new());
    }
    let mut lines: Lines = file
        .hunks
        .iter()
        .map(|h| vec![Vec::new(); h.lines.len()])
        .collect();
    for old in [true, false] {
        let mut source = String::new();
        let mut offsets = Vec::new();
        for (h, hunk) in file.hunks.iter().enumerate() {
            for (l, line) in hunk.lines.iter().enumerate() {
                if (old && line.kind == DiffLineKind::Addition)
                    || (!old && line.kind == DiffLineKind::Deletion)
                {
                    continue;
                }
                offsets.push((source.len(), h, l));
                source.push_str(&line.text);
            }
        }
        let spans = Highlighter::php()?.highlight(&source)?;
        for (start, h, l) in offsets {
            if old && file.hunks[h].lines[l].kind != DiffLineKind::Deletion {
                continue;
            }
            let end = start
                + file.hunks[h].lines[l]
                    .text
                    .trim_end_matches(['\r', '\n'])
                    .len();
            let first = spans.partition_point(|s| s.range.end <= start);
            lines[h][l] = spans[first..]
                .iter()
                .take_while(|s| s.range.start < end)
                .map(|s| HighlightSpan {
                    range: s.range.start.max(start) - start..s.range.end.min(end) - start,
                    token: s.token.clone(),
                })
                .collect();
        }
    }
    Ok(lines)
}
pub(super) fn job(text: &str, font: &FontId, spans: &[HighlightSpan], palette: &str) -> LayoutJob {
    let mut job = LayoutJob::default();
    let mut offset = 0;
    for span in spans {
        if span.range.start < offset
            || span.range.end > text.len()
            || !text.is_char_boundary(span.range.start)
            || !text.is_char_boundary(span.range.end)
        {
            continue;
        }
        append(
            &mut job,
            &text[offset..span.range.start],
            font,
            theme::TEXT,
            false,
        );
        let color = if palette == "plain" {
            theme::TEXT
        } else if palette == "muted" {
            theme::syntax_color(&span.token).gamma_multiply(0.75)
        } else {
            theme::syntax_color(&span.token)
        };
        append(
            &mut job,
            &text[span.range.clone()],
            font,
            color,
            span.token == "comment" && palette != "plain",
        );
        offset = span.range.end;
    }
    append(&mut job, &text[offset..], font, theme::TEXT, false);
    job
}
fn append(job: &mut LayoutJob, text: &str, font: &FontId, color: egui::Color32, italics: bool) {
    job.append(
        text,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color,
            italics,
            ..Default::default()
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_vcs::model::{DiffHunk, DiffLine};

    #[test]
    fn old_new_multiline_context_unicode_and_palettes() -> Result<(), Box<dyn std::error::Error>> {
        let rows = [
            (DiffLineKind::Context, "<?php\n"),
            (DiffLineKind::Context, "/* comment begins\n"),
            (DiffLineKind::Deletion, "old comment 🦀\n"),
            (DiffLineKind::Addition, "new comment Привет\n"),
            (DiffLineKind::Context, "*/\n"),
            (DiffLineKind::Deletion, "$old = 'before';\n"),
            (DiffLineKind::Addition, "$new = 'after';\n"),
        ];
        let file = FileDiff {
            path: "test.PHTML".into(),
            full_hunks: None,
            added: 2,
            deleted: 2,
            binary: false,
            truncated: false,
            hunks: vec![DiffHunk {
                header: String::new(),
                old_start: 1,
                new_start: 1,
                old_len: 5,
                new_len: 5,
                lines: rows
                    .into_iter()
                    .map(|(kind, text)| DiffLine {
                        kind,
                        text: text.into(),
                        old_line: None,
                        new_line: None,
                    })
                    .collect(),
            }],
        };
        let lines = highlight(&file)?;
        for row in [2, 3] {
            assert!(lines[0][row].iter().any(|s| s.token == "comment"));
        }
        for row in [5, 6] {
            assert!(lines[0][row].iter().any(|s| s.token.starts_with("string")));
            let text = file.hunks[0].lines[row].text.trim_end();
            let colored = job(text, &FontId::monospace(14.0), &lines[0][row], "default");
            assert_eq!(colored.text, text);
            assert!(
                colored
                    .sections
                    .iter()
                    .any(|s| s.format.color != theme::TEXT)
            );
            let plain = job(text, &FontId::monospace(14.0), &lines[0][row], "plain");
            assert!(plain.sections.iter().all(|s| s.format.color == theme::TEXT));
        }
        let mut state = Syntax::default();
        let config = nozhik_config::Config {
            enabled_languages: std::collections::BTreeSet::new(),
            ..Default::default()
        };
        state.configure(&config, Some(&file));
        state.poll(&egui::Context::default(), Some(&file));
        assert!(!state.started && state.receiver.is_none());
        Ok(())
    }
}
