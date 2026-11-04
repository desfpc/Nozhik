//! Project-local scratch tools. JSON parsing and formatting belong to a worker.
use egui::{Context, FontId, TextFormat};
use nozhik_language_support::{highlighter::HighlightSpan, json};
use std::sync::mpsc;

const LIMIT: usize = 2 * 1024 * 1024;
struct Rendered {
    text: [String; 2],
    spans: [Vec<HighlightSpan>; 2],
}
struct Reply {
    revision: u64,
    side: usize,
    original: Vec<HighlightSpan>,
    result: Result<Rendered, String>,
}
struct Worker {
    input: mpsc::SyncSender<(u64, usize, String)>,
    output: mpsc::Receiver<Reply>,
}
#[derive(Default)]
pub struct Tools {
    pub json_open: bool,
    text: [String; 2],
    spans: [Vec<HighlightSpan>; 2],
    revision: u64,
    error: Option<String>,
    worker: Option<Worker>,
    pending: Option<(u64, usize, String)>,
    reset_view: bool,
    split: f32,
    #[cfg(test)]
    geometry: [Option<(egui::Rect, egui::Pos2, std::sync::Arc<egui::Galley>)>; 2],
}
impl Tools {
    pub fn load(&mut self, source: String) {
        self.json_open = true;
        self.reset_view = true;
        self.text[0] = source;
        self.changed(0);
    }
    fn changed(&mut self, side: usize) {
        self.revision += 1;
        self.spans[side].clear();
        self.error = None;
        if self.text[side].len() > LIMIT {
            self.pending = None;
            self.error = Some("JSON tool limit: 2 MiB. Reduce the input to continue.".into());
        } else {
            self.pending = Some((self.revision, side, self.text[side].clone()));
        }
    }
    fn tick(&mut self, ctx: &Context) {
        if self.worker.is_none() {
            let (input, jobs) = mpsc::sync_channel::<(u64, usize, String)>(1);
            let (results, output) = mpsc::channel();
            let wake = ctx.clone();
            std::thread::spawn(move || {
                while let Ok((revision, side, source)) = jobs.recv() {
                    let original = json::highlight(&source);
                    let result = json::render(&source, None)
                        .and_then(|compact| {
                            let pretty = json::render(&source, Some("  "))?;
                            let a = json::highlight(&compact);
                            let b = json::highlight(&pretty);
                            Ok(Rendered {
                                text: [compact, pretty],
                                spans: [a, b],
                            })
                        })
                        .map_err(|e| e.to_string());
                    if results
                        .send(Reply {
                            revision,
                            side,
                            original,
                            result,
                        })
                        .is_err()
                    {
                        break;
                    }
                    wake.request_repaint();
                }
            });
            self.worker = Some(Worker { input, output });
        }
        let Some(worker) = &self.worker else {
            return;
        };
        while let Ok(reply) = worker.output.try_recv() {
            if reply.revision != self.revision {
                continue;
            }
            match reply.result {
                Ok(rendered) => {
                    let replacements = rendered.text;
                    for (side, new) in replacements.into_iter().enumerate() {
                        if side == reply.side {
                            remap_cursor(
                                ctx,
                                side,
                                &self.text[side],
                                &new,
                                &reply.original,
                                &rendered.spans[side],
                            );
                        }
                        self.text[side] = new;
                    }
                    self.spans = rendered.spans;
                    self.error = None;
                }
                Err(error) => {
                    self.spans[reply.side] = reply.original;
                    self.error = Some(error);
                }
            }
        }
        if let Some(job) = self.pending.take() {
            match worker.input.try_send(job) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(job)) => {
                    self.pending = Some(job);
                    ctx.request_repaint_after(std::time::Duration::from_millis(30));
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    self.error = Some("JSON worker stopped".into());
                }
            }
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.json_open && ui.button("Back to Tools").clicked() {
                self.json_open = false;
            }
            ui.heading(if self.json_open {
                "Tools / JSON"
            } else {
                "Tools"
            });
        });
        ui.separator();
        if !self.json_open {
            if ui
                .add_sized([310.0, 48.0], egui::Button::new("{}  JSON"))
                .clicked()
            {
                self.json_open = true;
            }
            ui.weak("Format, minify and edit JSON in two synchronized panes.");
            return;
        }
        self.tick(ui.ctx());
        ui.horizontal_wrapped(|ui| {
            if ui.button("Clear").clicked() {
                self.text = Default::default();
                self.spans = Default::default();
                self.revision += 1;
                self.pending = None;
                self.error = None;
                self.reset_view = true;
            }
            ui.weak("Edit either pane · valid JSON formats automatically · 2-space indent");
        });
        if let Some(error) = &self.error {
            ui.colored_label(crate::theme::WARNING, error);
        }
        let height = (ui.available_height() - 10.0).max(80.0);
        if self.split == 0.0 {
            self.split = 0.3;
        }
        let sizes = [height * self.split, height * (1.0 - self.split)];
        if self.reset_view {
            for side in [0, 1] {
                let mut state =
                    egui::TextEdit::load_state(ui.ctx(), editor_id(side)).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(0),
                    )));
                state.clear_undoer();
                state.store(ui.ctx(), editor_id(side));
            }
        }
        for (side, pane_height) in sizes.into_iter().enumerate() {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), pane_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.pane(ui, side, pane_height),
            );
            if side == 0 {
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), 6.0),
                    egui::Sense::drag(),
                );
                response
                    .clone()
                    .on_hover_cursor(egui::CursorIcon::ResizeVertical);
                ui.painter().hline(
                    rect.x_range(),
                    rect.center().y,
                    egui::Stroke::new(1.0_f32, crate::theme::BORDER),
                );
                if response.dragged() {
                    self.split =
                        (self.split + response.drag_delta().y / height.max(1.0)).clamp(0.15, 0.7);
                }
            }
        }
        self.reset_view = false;
        self.tick(ui.ctx());
    }
    fn pane(&mut self, ui: &mut egui::Ui, side: usize, height: f32) {
        ui.horizontal(|ui| {
            ui.label(if side == 0 {
                "Minified JSON"
            } else {
                "Formatted JSON"
            });
            if ui.button("Copy").clicked() {
                ui.ctx().copy_text(self.text[side].clone());
            }
        });
        let pane_size = egui::vec2(ui.available_width(), (height - 32.0).max(20.0));
        let spans = &self.spans[side];
        let mut layouter = |ui: &egui::Ui, text: &dyn egui::TextBuffer, _width: f32| {
            ui.fonts(|f| f.layout_job(layout_json(text.as_str(), spans)))
        };
        let mut scroll = egui::ScrollArea::both()
            .id_salt(("json_scroll", side))
            .auto_shrink([false, false])
            .max_width(pane_size.x)
            .max_height(pane_size.y);
        if self.reset_view {
            scroll = scroll.scroll_offset(egui::Vec2::ZERO);
        }
        let output = scroll.show(ui, |ui| {
            // add_sized centers overflowing widgets and clips the start of long JSON.
            // Allocate from the top left and let ScrollArea contain the overflow.
            egui::TextEdit::multiline(&mut self.text[side])
                .id(editor_id(side))
                .code_editor()
                .desired_width(pane_size.x)
                .min_size(pane_size)
                .horizontal_align(egui::Align::Min)
                .layouter(&mut layouter)
                .show(ui)
        });
        #[cfg(test)]
        {
            self.geometry[side] = Some((
                output.inner_rect,
                output.inner.galley_pos,
                output.inner.galley.clone(),
            ));
        }
        if output.inner.response.changed() {
            self.changed(side);
        }
    }
}
fn layout_json(text: &str, spans: &[HighlightSpan]) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let mut start = 0;
    for span in spans {
        if span.range.start < start
            || !text.is_char_boundary(span.range.start)
            || !text.is_char_boundary(span.range.end)
        {
            continue;
        }
        append(&mut job, &text[start..span.range.start], crate::theme::TEXT);
        append(
            &mut job,
            &text[span.range.clone()],
            crate::theme::syntax_color(&span.token),
        );
        start = span.range.end;
    }
    append(&mut job, &text[start..], crate::theme::TEXT);
    job
}

fn editor_id(side: usize) -> egui::Id {
    egui::Id::new(("json_tool_editor", side))
}
fn append(job: &mut egui::text::LayoutJob, text: &str, color: egui::Color32) {
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: FontId::monospace(14.0),
            color,
            ..Default::default()
        },
    );
}
fn remap_cursor(
    ctx: &Context,
    side: usize,
    old: &str,
    new: &str,
    before: &[HighlightSpan],
    after: &[HighlightSpan],
) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, editor_id(side)) {
        if let Some(mut range) = state.cursor.char_range() {
            range.primary.index = map_position(old, new, range.primary.index, before, after);
            range.secondary.index = map_position(old, new, range.secondary.index, before, after);
            state.cursor.set_char_range(Some(range));
        }
        state.store(ctx, editor_id(side));
    }
}
fn map_position(
    old: &str,
    new: &str,
    position: usize,
    before: &[HighlightSpan],
    after: &[HighlightSpan],
) -> usize {
    let byte = old
        .char_indices()
        .nth(position)
        .map_or(old.len(), |(i, _)| i);
    for (a, b) in before.iter().zip(after) {
        if byte <= a.range.end {
            let offset = byte.saturating_sub(a.range.start).min(b.range.len());
            return new[..b.range.start + offset].chars().count();
        }
    }
    new.chars().count()
}
#[cfg(test)]
mod tests {
    use super::*;

    fn wait(
        tools: &mut Tools,
        ctx: &Context,
        ready: impl Fn(&Tools) -> bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            tools.tick(ctx);
            if ready(tools) {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err("JSON synchronization timeout".into());
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }
    #[test]
    fn both_editors_sync_invalid_text_and_stale_results() -> Result<(), Box<dyn std::error::Error>>
    {
        let ctx = Context::default();
        let mut tools = Tools::default();
        tools.load(r#"{ "a": [1, true] }"#.into());
        assert!(tools.json_open, "file transfer opens the tool directly");
        wait(&mut tools, &ctx, |t| t.text[1].contains("\n  \"a\": ["))?;
        assert_eq!(tools.text[0], r#"{"a":[1,true]}"#);
        tools.text[1] = r#"{ "ключ": "new value" }"#.into();
        tools.changed(1);
        wait(&mut tools, &ctx, |t| t.text[0].contains("new value"))?;
        assert_eq!(tools.text[0], r#"{"ключ":"new value"}"#);
        let valid = tools.text[0].clone();
        tools.text[1] = "{\"unfinished\":".into();
        tools.changed(1);
        wait(&mut tools, &ctx, |t| t.error.is_some())?;
        assert_eq!(tools.text[1], "{\"unfinished\":");
        assert_eq!(tools.text[0], valid);
        tools.text[0] = "[1]".into();
        tools.changed(0);
        tools.tick(&ctx);
        tools.text[1] = "[2]".into();
        tools.changed(1);
        wait(&mut tools, &ctx, |t| t.text[0] == "[2]")?;
        assert!(tools.error.is_none());
        assert!(!tools.spans[1].is_empty());
        tools.text[0] = " ".repeat(LIMIT + 1);
        tools.changed(0);
        assert!(tools.error.as_ref().is_some_and(|e| e.contains("2 MiB")));
        assert!(tools.pending.is_none());
        Ok(())
    }
    #[test]
    fn catalog_click_opens_full_size_json_and_back() -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Context::default();
        let mut tools = Tools::default();
        let frame = |tools: &mut Tools, events| {
            ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1400.0, 900.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| tools.show(ui));
                },
            )
        };
        frame(&mut tools, vec![]);
        let output = frame(&mut tools, vec![]);
        assert!(!tools.json_open);
        let button = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(t) if t.galley.text() == "{}  JSON" => {
                    Some(t.pos + t.galley.size() * 0.5)
                }
                _ => None,
            })
            .ok_or("JSON catalog button")?;
        for pressed in [true, false] {
            frame(
                &mut tools,
                vec![
                    egui::Event::PointerMoved(button),
                    egui::Event::PointerButton {
                        pos: button,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(tools.json_open);
        let output = frame(&mut tools, vec![]);
        assert!(
            ctx.memory(|m| m.area_rect(egui::Id::new("json_tool_window")))
                .is_none()
        );
        let back = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(t) if t.galley.text() == "Back to Tools" => {
                    Some(t.pos + t.galley.size() * 0.5)
                }
                _ => None,
            })
            .ok_or("back button")?;
        for pressed in [true, false] {
            frame(
                &mut tools,
                vec![
                    egui::Event::PointerMoved(back),
                    egui::Event::PointerButton {
                        pos: back,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(!tools.json_open);
        Ok(())
    }

    #[test]
    fn long_json_panes_are_stacked_and_start_inside_their_clip_rects()
    -> Result<(), Box<dyn std::error::Error>> {
        let mut tools = Tools::default();
        let source = format!(
            "{{\"first_key\":\"{}\",\"rows\":[{}]}}",
            "long text ".repeat(120),
            (0..80)
                .map(|i| format!("{{\"id\":{i}}}"))
                .collect::<Vec<_>>()
                .join(",")
        );
        let ctx = Context::default();
        tools.load(source);
        wait(&mut tools, &ctx, |t| !t.text[1].is_empty())?;
        for size in [egui::vec2(1400.0, 900.0), egui::vec2(800.0, 600.0)] {
            for _ in 0..3 {
                let _output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| tools.show(ui));
                    },
                );
            }
            let (top, top_pos, top_text) = tools.geometry[0].as_ref().ok_or("minified geometry")?;
            let (bottom, bottom_pos, bottom_text) =
                tools.geometry[1].as_ref().ok_or("formatted geometry")?;
            assert!(top.bottom() < bottom.top());
            assert!((top.left() - bottom.left()).abs() < 1.0);
            assert!((top.width() - bottom.width()).abs() < 1.0);
            for (rect, position, text) in
                [(top, top_pos, top_text), (bottom, bottom_pos, bottom_text)]
            {
                assert!(
                    rect.contains(*position),
                    "JSON beginning outside viewport: {position:?}, {rect:?}"
                );
                assert!(
                    rect.bottom() <= size.y,
                    "pane extends below workspace: {rect:?}"
                );
                assert!(text.text().starts_with('{'));
            }
        }
        Ok(())
    }

    fn label(output: &egui::FullOutput, name: &str) -> Option<egui::Pos2> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(t) if t.galley.text() == name => Some(t.pos + t.galley.size() * 0.5),
            _ => None,
        })
    }
    fn shell_frame(
        ctx: &Context,
        shell: &mut crate::NozhikUi,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1400.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                shell.panels(ctx);
                shell.editor_area(ctx);
            },
        )
    }
    #[test]
    fn file_transfer_and_keyboard_edits_copy_without_writing_source()
    -> Result<(), Box<dyn std::error::Error>> {
        use crate::{Document, NozhikUi};
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("sample.json");
        let saved = r#"{"saved":true}"#;
        std::fs::write(&path, saved)?;
        let source = r#"{"unsaved":"значение"}"#;
        let bus = nozhik_event_bus::EventBus::new(32)?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
        let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
        shell.documents.push(Document {
            path: path.clone(),
            binary: false,
            name: "sample.json".into(),
            open: true,
            buffer: nozhik_buffer::TextBuffer::load_from_str(nozhik_core::BufferId(90), source),
        });
        shell.active = Some(0);
        let ctx = Context::default();
        shell_frame(&ctx, &mut shell, vec![]);
        let output = shell_frame(&ctx, &mut shell, vec![]);
        let transfer = label(&output, "Open in Tools / JSON").ok_or("transfer button")?;
        for pressed in [true, false] {
            shell_frame(
                &ctx,
                &mut shell,
                vec![
                    egui::Event::PointerMoved(transfer),
                    egui::Event::PointerButton {
                        pos: transfer,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(shell.tools.json_open && shell.left == Some(crate::LeftTool::Tools));
        assert!(
            !shell.action_context().editor_open,
            "Tools must not target the hidden file editor"
        );
        wait(&mut shell.tools, &ctx, |t| t.text[1].contains("значение"))?;
        for side in [0, 1] {
            shell_frame(&ctx, &mut shell, vec![]);
            ctx.memory_mut(|m| m.request_focus(editor_id(side)));
            let mut state =
                egui::TextEdit::load_state(&ctx, editor_id(side)).ok_or("editor state")?;
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(shell.tools.text[side].chars().count()),
                )));
            state.store(&ctx, editor_id(side));
            let input = format!("{{\"side\":{side}}}");
            shell_frame(&ctx, &mut shell, vec![egui::Event::Paste(input.clone())]);
            wait(&mut shell.tools, &ctx, |t| t.text[0] == input)?;
        }
        let output = shell_frame(&ctx, &mut shell, vec![]);
        let copy = label(&output, "Copy").ok_or("copy button")?;
        shell_frame(
            &ctx,
            &mut shell,
            vec![
                egui::Event::PointerMoved(copy),
                egui::Event::PointerButton {
                    pos: copy,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        let output = shell_frame(
            &ctx,
            &mut shell,
            vec![egui::Event::PointerButton {
                pos: copy,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        assert!(
            output
                .platform_output
                .commands
                .iter()
                .any(|command| matches!(command,
            egui::OutputCommand::CopyText(text) if text == r#"{"side":1}"#))
        );
        assert_eq!(std::fs::read_to_string(path)?, saved);
        assert_eq!(shell.documents[0].buffer.rope().to_string(), source);
        Ok(())
    }
    #[test]
    fn cursor_tracks_unicode_strings_and_whitespace() -> Result<(), Box<dyn std::error::Error>> {
        let old = r#"{"ключ":"a b","n":1}"#;
        let new = json::render(old, Some("  "))?;
        let caret = old.find('b').ok_or("b")?;
        let mapped = map_position(
            old,
            &new,
            old[..caret].chars().count(),
            &json::highlight(old),
            &json::highlight(&new),
        );
        assert_eq!(new.chars().nth(mapped), Some('b'));
        Ok(())
    }
}
