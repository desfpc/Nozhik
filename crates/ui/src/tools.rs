//! Project-local tools with independent scratch buffers and background transformations.
mod codec;
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
#[derive(Debug)]
struct Job {
    revision: u64,
    side: usize,
    source: String,
    form_url: bool,
}
struct Worker {
    input: mpsc::SyncSender<Job>,
    output: mpsc::Receiver<Reply>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum ToolKind {
    Json,
    Url,
    Base64,
}
impl ToolKind {
    const ALL: [Self; 3] = [Self::Json, Self::Url, Self::Base64];
    const fn index(self) -> usize {
        match self {
            Self::Json => 0,
            Self::Url => 1,
            Self::Base64 => 2,
        }
    }
    const fn title(self) -> &'static str {
        match self {
            Self::Json => "JSON",
            Self::Url => "URL encode / decode",
            Self::Base64 => "Base64 encode / decode",
        }
    }
    const fn button(self) -> &'static str {
        match self {
            Self::Json => "{}  JSON",
            Self::Url => "URL encode / decode",
            Self::Base64 => "Base64 encode / decode",
        }
    }
    const fn hint(self) -> &'static str {
        match self {
            Self::Json => "Edit either pane · valid JSON formats automatically · 2-space indent",
            Self::Url => "Edit either pane · UTF-8 percent encoding · spaces use %20 by default",
            Self::Base64 => "Edit either pane · standard Base64 · decoded text is UTF-8",
        }
    }
    const fn pane_label(self, side: usize) -> &'static str {
        match (self, side) {
            (Self::Json, 0) => "Minified JSON",
            (Self::Json, _) => "Formatted JSON",
            (_, 0) => "Decoded text",
            (Self::Url, _) => "URL-encoded text",
            (Self::Base64, _) => "Base64-encoded text",
        }
    }
}
#[derive(Default)]
pub struct Tools {
    selected: Option<ToolKind>,
    editors: [ToolEditor; 3],
}
impl Tools {
    pub const fn catalog(&mut self) {
        self.selected = None;
    }
    pub fn load(&mut self, source: String) {
        self.selected = Some(ToolKind::Json);
        self.editors[ToolKind::Json.index()].load(source);
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if self.selected.is_some() && ui.button("Back to Tools").clicked() {
                self.catalog();
            }
            ui.heading(self.selected.map_or_else(
                || "Tools".into(),
                |kind| format!("Tools / {}", kind.title()),
            ));
        });
        ui.separator();
        if let Some(kind) = self.selected {
            self.editors[kind.index()].show(ui, kind);
        } else {
            for kind in ToolKind::ALL {
                if ui
                    .add_sized([310.0, 48.0], egui::Button::new(kind.button()))
                    .clicked()
                {
                    self.selected = Some(kind);
                }
            }
            ui.weak("Choose a tool. Both panes are editable and stay synchronized.");
        }
    }
}
fn transform(
    kind: ToolKind,
    side: usize,
    source: &str,
    form_url: bool,
) -> Result<Rendered, String> {
    if kind == ToolKind::Json {
        let compact = json::render(source, None).map_err(|e| e.to_string())?;
        let pretty = json::render(source, Some("  ")).map_err(|e| e.to_string())?;
        let spans = [json::highlight(&compact), json::highlight(&pretty)];
        return Ok(Rendered {
            text: [compact, pretty],
            spans,
        });
    }
    let converted = codec::convert(kind, side, source, form_url)?;
    let text = if side == 0 {
        [source.into(), converted]
    } else {
        [converted, source.into()]
    };
    Ok(Rendered {
        text,
        spans: Default::default(),
    })
}
#[derive(Default)]
struct ToolEditor {
    text: [String; 2],
    spans: [Vec<HighlightSpan>; 2],
    revision: u64,
    error: Option<String>,
    worker: Option<Worker>,
    pending: Option<Job>,
    form_url: bool,
    last_side: usize,
    reset_view: bool,
    split: f32,
    #[cfg(test)]
    geometry: [Option<(egui::Rect, egui::Pos2, std::sync::Arc<egui::Galley>)>; 2],
}
impl ToolEditor {
    fn load(&mut self, source: String) {
        self.reset_view = true;
        self.text[0] = source;
        self.changed(0);
    }
    fn changed(&mut self, side: usize) {
        self.revision += 1;
        self.last_side = side;
        self.spans[side].clear();
        self.error = None;
        if self.text[side].len() > LIMIT {
            self.pending = None;
            self.error = Some("Tool input limit: 2 MiB. Reduce the input to continue.".into());
        } else {
            self.pending = Some(Job {
                revision: self.revision,
                side,
                source: self.text[side].clone(),
                form_url: self.form_url,
            });
        }
    }
    fn tick(&mut self, ctx: &Context, kind: ToolKind) {
        if self.worker.is_none() {
            let (input, jobs) = mpsc::sync_channel::<Job>(1);
            let (results, output) = mpsc::channel();
            let wake = ctx.clone();
            std::thread::spawn(move || {
                while let Ok(Job {
                    revision,
                    side,
                    source,
                    form_url,
                }) = jobs.recv()
                {
                    let original = if kind == ToolKind::Json {
                        json::highlight(&source)
                    } else {
                        Vec::new()
                    };
                    let result = transform(kind, side, &source, form_url);
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
                        if side == reply.side && kind == ToolKind::Json {
                            remap_cursor(
                                ctx,
                                editor_id(kind, side),
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
                    self.error = Some("Tool worker stopped".into());
                }
            }
        }
    }
    fn show(&mut self, ui: &mut egui::Ui, kind: ToolKind) {
        self.tick(ui.ctx(), kind);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Clear").clicked() {
                self.text = Default::default();
                self.spans = Default::default();
                self.revision += 1;
                self.pending = None;
                self.error = None;
                self.reset_view = true;
            }
            ui.weak(kind.hint());
            if kind == ToolKind::Url
                && ui
                    .checkbox(&mut self.form_url, "Form encoding (+ for spaces)")
                    .changed()
            {
                self.changed(self.last_side);
            }
        });
        if let Some(error) = &self.error {
            ui.colored_label(crate::theme::warning(), error);
        }
        let height = (ui.available_height() - 10.0).max(80.0);
        if self.split == 0.0 {
            self.split = if kind == ToolKind::Json { 0.3 } else { 0.5 };
        }
        let sizes = [height * self.split, height * (1.0 - self.split)];
        if self.reset_view {
            for side in [0, 1] {
                let mut state =
                    egui::TextEdit::load_state(ui.ctx(), editor_id(kind, side)).unwrap_or_default();
                state
                    .cursor
                    .set_char_range(Some(egui::text::CCursorRange::one(
                        egui::text::CCursor::new(0),
                    )));
                state.clear_undoer();
                state.store(ui.ctx(), editor_id(kind, side));
            }
        }
        for (side, pane_height) in sizes.into_iter().enumerate() {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), pane_height),
                egui::Layout::top_down(egui::Align::Min),
                |ui| self.pane(ui, kind, side, pane_height),
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
                    egui::Stroke::new(1.0_f32, crate::theme::border()),
                );
                if response.dragged() {
                    self.split =
                        (self.split + response.drag_delta().y / height.max(1.0)).clamp(0.15, 0.7);
                }
            }
        }
        self.reset_view = false;
        self.tick(ui.ctx(), kind);
    }
    fn pane(&mut self, ui: &mut egui::Ui, kind: ToolKind, side: usize, height: f32) {
        ui.horizontal(|ui| {
            ui.label(kind.pane_label(side));
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
            .id_salt(("tool_scroll", kind, side))
            .auto_shrink([false, false])
            .max_width(pane_size.x)
            .max_height(pane_size.y);
        if self.reset_view {
            scroll = scroll.scroll_offset(egui::Vec2::ZERO);
        }
        let output = scroll.show(ui, |ui| {
            // add_sized centers overflowing widgets and clips the start of long text.
            // Allocate from the top left and let ScrollArea contain the overflow.
            egui::TextEdit::multiline(&mut self.text[side])
                .id(editor_id(kind, side))
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
        append(
            &mut job,
            &text[start..span.range.start],
            crate::theme::text(),
        );
        append(
            &mut job,
            &text[span.range.clone()],
            crate::theme::syntax_color(&span.token),
        );
        start = span.range.end;
    }
    append(&mut job, &text[start..], crate::theme::text());
    job
}

fn editor_id(kind: ToolKind, side: usize) -> egui::Id {
    egui::Id::new(("tool_editor", kind, side))
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
    id: egui::Id,
    old: &str,
    new: &str,
    before: &[HighlightSpan],
    after: &[HighlightSpan],
) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        if let Some(mut range) = state.cursor.char_range() {
            range.primary.index = map_position(old, new, range.primary.index, before, after);
            range.secondary.index = map_position(old, new, range.secondary.index, before, after);
            state.cursor.set_char_range(Some(range));
        }
        state.store(ctx, id);
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
        ready: impl Fn(&ToolEditor) -> bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        wait_tool(tools, ctx, ToolKind::Json, ready)
    }
    fn wait_tool(
        tools: &mut Tools,
        ctx: &Context,
        kind: ToolKind,
        ready: impl Fn(&ToolEditor) -> bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let editor = &mut tools.editors[kind.index()];
            editor.tick(ctx, kind);
            if ready(editor) {
                return Ok(());
            }
            if std::time::Instant::now() >= deadline {
                return Err(format!(
                    "Tool synchronization timeout: {kind:?}, text={:?}, error={:?}",
                    editor.text, editor.error
                )
                .into());
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
        assert_eq!(
            tools.selected,
            Some(ToolKind::Json),
            "file transfer opens the tool directly"
        );
        wait(&mut tools, &ctx, |t| t.text[1].contains("\n  \"a\": ["))?;
        assert_eq!(tools.editors[0].text[0], r#"{"a":[1,true]}"#);
        tools.editors[0].text[1] = r#"{ "ключ": "new value" }"#.into();
        tools.editors[0].changed(1);
        wait(&mut tools, &ctx, |t| t.text[0].contains("new value"))?;
        assert_eq!(tools.editors[0].text[0], r#"{"ключ":"new value"}"#);
        let valid = tools.editors[0].text[0].clone();
        tools.editors[0].text[1] = "{\"unfinished\":".into();
        tools.editors[0].changed(1);
        wait(&mut tools, &ctx, |t| t.error.is_some())?;
        assert_eq!(tools.editors[0].text[1], "{\"unfinished\":");
        assert_eq!(tools.editors[0].text[0], valid);
        tools.editors[0].text[0] = "[1]".into();
        tools.editors[0].changed(0);
        tools.editors[0].tick(&ctx, ToolKind::Json);
        tools.editors[0].text[1] = "[2]".into();
        tools.editors[0].changed(1);
        wait(&mut tools, &ctx, |t| t.text[0] == "[2]")?;
        assert!(tools.editors[0].error.is_none());
        assert!(!tools.editors[0].spans[1].is_empty());
        tools.editors[0].text[0] = " ".repeat(LIMIT + 1);
        tools.editors[0].changed(0);
        assert!(
            tools.editors[0]
                .error
                .as_ref()
                .is_some_and(|e| e.contains("2 MiB"))
        );
        assert!(tools.editors[0].pending.is_none());
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
        assert!(tools.selected.is_none());
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
        assert_eq!(tools.selected, Some(ToolKind::Json));
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
        assert!(tools.selected.is_none());
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
            let (top, top_pos, top_text) = tools.editors[0].geometry[0]
                .as_ref()
                .ok_or("minified geometry")?;
            let (bottom, bottom_pos, bottom_text) = tools.editors[0].geometry[1]
                .as_ref()
                .ok_or("formatted geometry")?;
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
        let kind = ToolKind::Json;
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
        assert!(
            shell.tools.selected == Some(ToolKind::Json)
                && shell.left == Some(crate::LeftTool::Tools)
        );
        assert!(
            !shell.action_context().editor_open,
            "Tools must not target the hidden file editor"
        );
        wait(&mut shell.tools, &ctx, |t| t.text[1].contains("значение"))?;
        for side in [0, 1] {
            shell_frame(&ctx, &mut shell, vec![]);
            ctx.memory_mut(|m| m.request_focus(editor_id(kind, side)));
            let mut state =
                egui::TextEdit::load_state(&ctx, editor_id(kind, side)).ok_or("editor state")?;
            state
                .cursor
                .set_char_range(Some(egui::text::CCursorRange::two(
                    egui::text::CCursor::new(0),
                    egui::text::CCursor::new(shell.tools.editors[0].text[side].chars().count()),
                )));
            state.store(&ctx, editor_id(kind, side));
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

    fn tool_frame(ctx: &Context, tools: &mut Tools, events: Vec<egui::Event>) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1100.0, 760.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| tools.show(ui));
            },
        )
    }
    fn click_tool(ctx: &Context, tools: &mut Tools, pos: egui::Pos2) -> egui::FullOutput {
        tool_frame(
            ctx,
            tools,
            vec![
                egui::Event::PointerMoved(pos),
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        tool_frame(
            ctx,
            tools,
            vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        )
    }
    fn paste_tool(
        ctx: &Context,
        tools: &mut Tools,
        kind: ToolKind,
        side: usize,
        value: &str,
    ) -> Result<(), Box<dyn std::error::Error>> {
        tool_frame(ctx, tools, vec![]);
        let id = editor_id(kind, side);
        ctx.memory_mut(|m| m.request_focus(id));
        let mut state = egui::TextEdit::load_state(ctx, id).ok_or("editor state")?;
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(tools.editors[kind.index()].text[side].chars().count()),
            )));
        state.store(ctx, id);
        let event = if value.is_empty() {
            egui::Event::Key {
                key: egui::Key::Backspace,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }
        } else {
            egui::Event::Paste(value.into())
        };
        tool_frame(ctx, tools, vec![event]);
        Ok(())
    }
    #[test]
    fn codec_catalog_keyboard_both_directions_copy_errors_and_independent_buffers()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Context::default();
        let mut tools = Tools::default();
        tools.load(r#"{"keep":true}"#.into());
        wait(&mut tools, &ctx, |e| !e.text[1].is_empty())?;
        let json_text = tools.editors[0].text.clone();
        for (kind, plain, encoded, replacement, invalid) in [
            (
                ToolKind::Url,
                "Привет +/",
                "%D0%9F%D1%80%D0%B8%D0%B2%D0%B5%D1%82%20%2B%2F",
                "new%20text%2B",
                "%GG",
            ),
            (
                ToolKind::Base64,
                "Привет +/",
                "0J/RgNC40LLQtdGCICsv",
                "bmV3IHRleHQr",
                "!!!",
            ),
        ] {
            tools.catalog();
            tool_frame(&ctx, &mut tools, vec![]);
            let output = tool_frame(&ctx, &mut tools, vec![]);
            let button = label(&output, kind.button()).ok_or("codec catalog button")?;
            click_tool(&ctx, &mut tools, button);
            assert_eq!(tools.selected, Some(kind));
            paste_tool(&ctx, &mut tools, kind, 0, plain)?;
            wait_tool(&mut tools, &ctx, kind, |e| e.text[1] == encoded)?;
            paste_tool(&ctx, &mut tools, kind, 1, replacement)?;
            wait_tool(&mut tools, &ctx, kind, |e| e.text[0] == "new text+")?;
            for side in [0, 1] {
                let output = tool_frame(&ctx, &mut tools, vec![]);
                let positions: Vec<_> = output
                    .shapes
                    .iter()
                    .filter_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text() == "Copy" => {
                            Some(t.pos + t.galley.size() * 0.5)
                        }
                        _ => None,
                    })
                    .collect();
                let output =
                    click_tool(&ctx, &mut tools, *positions.get(side).ok_or("Copy button")?);
                assert!(output.platform_output.commands.iter().any(|command| matches!(command,
                    egui::OutputCommand::CopyText(text) if text == &tools.editors[kind.index()].text[side])));
            }
            paste_tool(&ctx, &mut tools, kind, 1, invalid)?;
            wait_tool(&mut tools, &ctx, kind, |e| e.error.is_some())?;
            assert_eq!(tools.editors[kind.index()].text[0], "new text+");
            assert_eq!(tools.editors[kind.index()].text[1], invalid);
            paste_tool(&ctx, &mut tools, kind, 0, "")?;
            wait_tool(&mut tools, &ctx, kind, |e| e.text == ["", ""])?;
            assert!(tools.editors[kind.index()].error.is_none());
            let top = tools.editors[kind.index()].geometry[0]
                .as_ref()
                .ok_or("top")?
                .0;
            let bottom = tools.editors[kind.index()].geometry[1]
                .as_ref()
                .ok_or("bottom")?
                .0;
            assert!(top.bottom() < bottom.top());
        }
        assert_eq!(tools.editors[0].text, json_text);
        Ok(())
    }
    #[test]
    fn form_mode_recomputes_latest_side_and_obsolete_replies_cannot_replace_edits()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = Context::default();
        let kind = ToolKind::Url;
        let mut tools = Tools {
            selected: Some(kind),
            ..Default::default()
        };
        paste_tool(&ctx, &mut tools, kind, 0, "a b+")?;
        wait_tool(&mut tools, &ctx, kind, |e| e.text[1] == "a%20b%2B")?;
        let output = tool_frame(&ctx, &mut tools, vec![]);
        let checkbox = label(&output, "Form encoding (+ for spaces)").ok_or("form option")?;
        click_tool(&ctx, &mut tools, checkbox);
        wait_tool(&mut tools, &ctx, kind, |e| e.text[1] == "a+b%2B")?;
        let editor = &mut tools.editors[kind.index()];
        editor.text[0] = "obsolete".into();
        editor.changed(0);
        editor.tick(&ctx, kind);
        editor.text[1] = "latest+value".into();
        editor.changed(1);
        tools.catalog();
        tools.selected = Some(ToolKind::Base64);
        wait_tool(&mut tools, &ctx, kind, |e| e.text[0] == "latest value")?;
        assert!(
            tools.editors[ToolKind::Base64.index()]
                .text
                .iter()
                .all(String::is_empty)
        );
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
