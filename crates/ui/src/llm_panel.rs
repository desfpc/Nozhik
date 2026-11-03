//! Project-local LLM terminals and background preparation of editor context.
use crate::{Document, terminal_panel::TerminalPanel};
use nozhik_buffer::BufferSnapshot;
use nozhik_terminal::{TerminalSessionId, agent::ForegroundAgent};
use std::{
    fmt::Write as _,
    io::Read as _,
    path::{Path, PathBuf},
    sync::mpsc,
};

pub const LLM_LABEL: &str = "LLM";
const MAX_FILE: u64 = 20 * 1024 * 1024;
const MAX_CONTEXT: u64 = 256 * 1024 * 1024;

enum Context {
    Files(Vec<(PathBuf, BufferSnapshot)>),
    Selection(PathBuf, BufferSnapshot),
    Attach(Vec<PathBuf>),
}
struct Request {
    id: TerminalSessionId,
    agent: ForegroundAgent,
    context: Context,
}
struct Prepared {
    id: TerminalSessionId,
    agent: ForegroundAgent,
    result: Result<Vec<String>, String>,
}
pub struct LlmPanel {
    terminal: TerminalPanel,
    requests: Option<mpsc::SyncSender<Request>>,
    results: mpsc::Receiver<Prepared>,
    worker: Option<std::thread::JoinHandle<()>>,
    pending: bool,
    message: String,
}
impl LlmPanel {
    pub fn new(ctx: &egui::Context) -> Self {
        let (requests, input) = mpsc::sync_channel::<Request>(1);
        let (output, results) = mpsc::channel();
        let wake = ctx.clone();
        let worker = std::thread::spawn(move || {
            let mut storage = Storage::default();
            while let Ok(request) = input.recv() {
                let result = storage.prepare(request.context);
                if output
                    .send(Prepared {
                        id: request.id,
                        agent: request.agent,
                        result,
                    })
                    .is_err()
                {
                    break;
                }
                wake.request_repaint();
            }
        });
        Self {
            terminal: TerminalPanel::llm(ctx),
            requests: Some(requests),
            results,
            worker: Some(worker),
            pending: false,
            message: String::new(),
        }
    }
    pub fn tick(&mut self) {
        if let Some(message) = self.terminal.tick() {
            self.message = message;
        }
        for prepared in self.results.try_iter() {
            self.pending = false;
            let result = prepared.result.and_then(|pastes| {
                for text in pastes {
                    self.terminal
                        .paste_context(prepared.id, prepared.agent, &text)?;
                }
                Ok(())
            });
            self.message = result.map_or_else(
                |e| e,
                |()| "Context added to the draft; review it and press Enter to send.".into(),
            );
        }
    }
    pub const fn hidden(&mut self) {
        self.terminal.hidden();
    }
    pub const fn focus(&mut self) {
        self.terminal.focus = true;
    }
    pub fn clipboard_action(&mut self, ctx: &egui::Context, action: &str) -> bool {
        self.terminal.clipboard_action(ctx, action)
    }
    pub fn shutdown(&mut self) {
        self.terminal.shutdown();
        self.requests.take();
        if let Some(worker) = self.worker.take() {
            let _joined = worker.join();
        }
    }
    fn enqueue(&mut self, context: Context) {
        let Some((id, agent)) = self.terminal.active_agent() else {
            return;
        };
        let result = self
            .requests
            .as_ref()
            .ok_or_else(|| "LLM console closed".to_owned())
            .and_then(|sender| {
                sender
                    .try_send(Request { id, agent, context })
                    .map_err(|e| e.to_string())
            });
        match result {
            Ok(()) => {
                self.pending = true;
                self.message = "Preparing context…".into();
            }
            Err(error) => self.message = error,
        }
    }
    #[allow(
        clippy::too_many_arguments,
        reason = "The toolbar reads project-local editor and remote-document identities without owning buffers"
    )]
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        root: Option<&Path>,
        font: f32,
        enabled: bool,
        documents: &[Document],
        active: Option<usize>,
        remote: &std::collections::HashMap<PathBuf, (String, String)>,
    ) -> bool {
        let area = ui.available_rect_before_wrap();
        let recognized = self.terminal.active_agent();
        let mut context = None;
        ui.horizontal_wrapped(|ui| {
            ui.strong(recognized.map_or("Agent: not detected", |(_, a)| a.agent.label()));
            if recognized.is_some() {
                ui.add_enabled_ui(enabled && !self.pending, |ui| {
                    if ui
                        .add_enabled(
                            documents.iter().any(|d| {
                                d.open
                                    && !d.binary
                                    && d.path.is_absolute()
                                    && !remote.contains_key(&d.path)
                            }),
                            egui::Button::new("Open files"),
                        )
                        .on_hover_text(
                            "Add snapshots of all open local text files, including unsaved edits",
                        )
                        .clicked()
                    {
                        context = Some(Context::Files(
                            documents
                                .iter()
                                .filter(|d| {
                                    d.open
                                        && !d.binary
                                        && d.path.is_absolute()
                                        && !remote.contains_key(&d.path)
                                })
                                .map(|d| (d.path.clone(), d.buffer.snapshot()))
                                .collect(),
                        ));
                    }
                    let document = active.and_then(|i| documents.get(i)).filter(|d| {
                        d.open && !d.binary && d.path.is_absolute() && !remote.contains_key(&d.path)
                    });
                    let selected = document.is_some_and(|d| {
                        d.buffer
                            .cursors()
                            .as_slice()
                            .iter()
                            .any(|c| c.anchor != c.head)
                    });
                    if ui
                        .add_enabled(selected, egui::Button::new("Selection"))
                        .on_hover_text("Add selected text, source path and exact coordinates")
                        .clicked()
                        && let Some(d) = document
                    {
                        context = Some(Context::Selection(d.path.clone(), d.buffer.snapshot()));
                    }
                    if ui
                        .button("Attach file…")
                        .on_hover_text(
                            "Attach a local file or screenshot; files may also be dropped here",
                        )
                        .clicked()
                    {
                        match crate::project_dialog::pick(crate::project_dialog::Picker::File, root)
                        {
                            Ok(Some(path)) => context = Some(Context::Attach(vec![path])),
                            Ok(None) => {}
                            Err(error) => self.message = error,
                        }
                    }
                });
            } else if cfg!(unix) {
                ui.weak("Run grok or codex in this local terminal.");
            } else {
                ui.weak("Agent detection currently requires macOS or Linux.");
            }
        });
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
        let (collapse, error) = self.terminal.show(ui, root, font, enabled);
        if let Some(error) = error {
            self.message = error;
        }
        if let Some(dropped) = self.dropped_context(ui, area, enabled, recognized.is_some()) {
            context = Some(dropped);
        }
        if let Some(context) = context {
            self.enqueue(context);
        }
        collapse
    }
    fn dropped_context(
        &mut self,
        ui: &egui::Ui,
        area: egui::Rect,
        enabled: bool,
        recognized: bool,
    ) -> Option<Context> {
        if !enabled || !ui.input(|i| i.pointer.hover_pos().is_some_and(|p| area.contains(p))) {
            return None;
        }
        let dropped = ui.input_mut(|i| std::mem::take(&mut i.raw.dropped_files));
        if dropped.is_empty() {
            return None;
        }
        if !recognized {
            self.message = "Start a supported local LLM before attaching files.".into();
        } else if self.pending {
            self.message =
                "Context is still being prepared; drop the files again when finished.".into();
        } else {
            let paths: Vec<_> = dropped.into_iter().filter_map(|f| f.path).collect();
            if paths.is_empty() {
                self.message = "Only local file drops are supported.".into();
            } else {
                return Some(Context::Attach(paths));
            }
        }
        None
    }
}
impl Drop for LlmPanel {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[derive(Default)]
struct Storage {
    directory: Option<tempfile::TempDir>,
    count: u64,
    bytes: u64,
}
impl Storage {
    fn path(&mut self, extension: &str, bytes: u64) -> Result<PathBuf, String> {
        if bytes > MAX_FILE || self.bytes.saturating_add(bytes) > MAX_CONTEXT {
            return Err("LLM context storage limit: 20 MiB per file, 256 MiB per console. Reopen the project to reset.".into());
        }
        if self.directory.is_none() {
            self.directory = Some(
                tempfile::Builder::new()
                    .prefix("nozhik-llm-")
                    .tempdir()
                    .map_err(|e| e.to_string())?,
            );
        }
        self.count += 1;
        self.bytes += bytes;
        let directory = self
            .directory
            .as_ref()
            .ok_or("LLM context directory unavailable")?;
        Ok(directory
            .path()
            .join(format!("context-{}.{}", self.count, extension)))
    }
    fn prepare(&mut self, context: Context) -> Result<Vec<String>, String> {
        match context {
            Context::Selection(path, snapshot) => Ok(vec![selection_context(&path, &snapshot)?]),
            Context::Files(files) => {
                if files.is_empty() {
                    return Err("No open local text files".into());
                }
                let mut text = String::from(
                    "\nNozhik editor context. Read these UTF-8 snapshots; they include unsaved edits. Original paths identify the files to edit.\n",
                );
                for (original, snapshot) in files {
                    let path = self.path("txt", snapshot.text.len_bytes() as u64)?;
                    let mut file = std::fs::File::create(&path).map_err(|e| e.to_string())?;
                    snapshot
                        .text
                        .write_to(&mut file)
                        .map_err(|e| e.to_string())?;
                    writeln!(
                        text,
                        "Original: {}\nRevision: {}\nSnapshot: {}",
                        original.display(),
                        snapshot.revision,
                        path.display()
                    )
                    .map_err(|e| e.to_string())?;
                }
                nozhik_terminal::agent::paste_bytes(&text).map_err(|e| e.to_string())?;
                Ok(vec![text])
            }
            Context::Attach(paths) => {
                if paths.is_empty() || paths.len() > 16 {
                    return Err("Attach between 1 and 16 files at once".into());
                }
                let mut pastes = Vec::new();
                for original in paths {
                    // Read from one open handle, rejecting directories/devices and bounding
                    // growth during a copy. Staged ASCII names avoid shell/path ambiguity.
                    if !std::fs::metadata(&original)
                        .map_err(|e| e.to_string())?
                        .is_file()
                    {
                        return Err("Only regular local files can be attached".into());
                    }
                    let source = std::fs::File::open(&original).map_err(|e| e.to_string())?;
                    let metadata = source.metadata().map_err(|e| e.to_string())?;
                    if !metadata.is_file() {
                        return Err("Only regular local files can be attached".into());
                    }
                    let extension = original
                        .extension()
                        .and_then(|s| s.to_str())
                        .unwrap_or("bin")
                        .to_ascii_lowercase();
                    let extension = if extension.len() <= 10
                        && extension.chars().all(|c| c.is_ascii_alphanumeric())
                    {
                        extension
                    } else {
                        "bin".into()
                    };
                    let path = self.path(&extension, metadata.len())?;
                    let mut target = std::fs::File::create(&path).map_err(|e| e.to_string())?;
                    let copied = std::io::copy(&mut source.take(MAX_FILE + 1), &mut target)
                        .map_err(|e| e.to_string())?;
                    if copied > MAX_FILE {
                        return Err("Attachment grew beyond 20 MiB".into());
                    }
                    let image = matches!(
                        extension.as_str(),
                        "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tiff"
                    );
                    let text = if image {
                        path.display().to_string()
                    } else {
                        format!(
                            "\nRead attached local file (original {}): {}\n",
                            original.display(),
                            path.display()
                        )
                    };
                    nozhik_terminal::agent::paste_bytes(&text).map_err(|e| e.to_string())?;
                    pastes.push(text);
                }
                Ok(pastes)
            }
        }
    }
}
fn selection_context(path: &Path, snapshot: &BufferSnapshot) -> Result<String, String> {
    let mut text = format!(
        "\nNozhik selection: {} (editor revision {}). Coordinates: 1-based lines and UTF-8 byte columns, end exclusive.\n",
        path.display(),
        snapshot.revision
    );
    let mut found = false;
    for cursor in snapshot.cursors.as_slice() {
        let (start, end) = if cursor.anchor.byte_offset <= cursor.head.byte_offset {
            (cursor.anchor, cursor.head)
        } else {
            (cursor.head, cursor.anchor)
        };
        if start == end {
            continue;
        }
        if text
            .len()
            .saturating_add(end.byte_offset - start.byte_offset)
            > 64 * 1024
        {
            return Err("Selection exceeds 64 KiB; attach the file instead".into());
        }
        found = true;
        let range = snapshot.text.byte_to_char(start.byte_offset)
            ..snapshot.text.byte_to_char(end.byte_offset);
        writeln!(
            text,
            "{}:{}–{}:{} (bytes {}..{})\n{}",
            start.line + 1,
            start.column + 1,
            end.line + 1,
            end.column + 1,
            start.byte_offset,
            end.byte_offset,
            snapshot.text.slice(range)
        )
        .map_err(|e| e.to_string())?;
        if text.len() > 64 * 1024 {
            return Err("Selection exceeds 64 KiB; attach the file instead".into());
        }
    }
    if !found {
        return Err("Select text in the editor first".into());
    }
    let text = text.replace("\r\n", "\n");
    nozhik_terminal::agent::paste_bytes(&text).map_err(|e| e.to_string())?;
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_buffer::TextBuffer;
    use nozhik_core::{BufferId, Selection};
    type TestResult = Result<(), Box<dyn std::error::Error>>;

    #[test]
    fn unsaved_snapshots_selection_coordinates_and_attachment_lifetime() -> TestResult {
        let root = tempfile::tempdir()?;
        let original = root.path().join("source 'Ж'.php");
        std::fs::write(&original, "disk version")?;
        let mut buffer = TextBuffer::load_from_str(BufferId(91), "one\r\nЖ🙂abc\r\n");
        buffer.insert(buffer.rope().len_bytes(), "unsaved")?;
        buffer.set_cursors(vec![Selection {
            anchor: buffer.position_at(11)?,
            head: buffer.position_at(5)?,
        }])?;
        let snapshot = buffer.snapshot();
        let text = selection_context(&original, &snapshot)?;
        assert!(text.contains("2:1–2:7 (bytes 5..11)"));
        assert!(text.contains("Ж🙂"));
        assert!(!text.contains('\r'));
        let mut storage = Storage::default();
        let context = storage.prepare(Context::Files(vec![(original.clone(), snapshot)]))?;
        assert!(
            context
                .first()
                .is_some_and(|s| s.contains("unsaved edits") && s.contains("Revision: 1"))
        );
        let directory = storage
            .directory
            .as_ref()
            .ok_or("no snapshot directory")?
            .path()
            .to_path_buf();
        assert_eq!(
            std::fs::read_to_string(directory.join("context-1.txt"))?,
            "one\r\nЖ🙂abc\r\nunsaved"
        );
        assert_eq!(std::fs::read_to_string(&original)?, "disk version");
        let image = root.path().join("screenshot with spaces.png");
        std::fs::write(&image, b"image fixture")?;
        let context = storage.prepare(Context::Attach(vec![image.clone()]))?;
        let staged = PathBuf::from(context.first().ok_or("no attachment")?);
        std::fs::remove_file(image)?;
        assert_eq!(std::fs::read(&staged)?, b"image fixture");
        assert!(staged.starts_with(&directory));
        drop(storage);
        assert!(!directory.exists());
        Ok(())
    }

    #[test]
    fn drops_are_scoped_to_enabled_console_and_require_an_agent() {
        let ctx = egui::Context::default();
        let mut panel = LlmPanel::new(&ctx);
        let path = PathBuf::from("/tmp/screenshot.png");
        let area = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(200.0, 100.0));
        let mut accepted = false;
        for (point, enabled, recognized, expected) in [
            (egui::pos2(250.0, 20.0), true, true, false),
            (egui::pos2(20.0, 20.0), false, true, false),
            (egui::pos2(20.0, 20.0), true, false, false),
            (egui::pos2(20.0, 20.0), true, true, true),
        ] {
            let _output = ctx.run(egui::RawInput {
                screen_rect: Some(area),
                events: vec![egui::Event::PointerMoved(point)],
                dropped_files: vec![egui::DroppedFile { path: Some(path.clone()), ..Default::default() }],
                ..Default::default()
            }, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    accepted = matches!(panel.dropped_context(ui, area, enabled, recognized), Some(Context::Attach(paths)) if paths == vec![path.clone()]);
                });
            });
            assert_eq!(accepted, expected);
        }
        assert!(!panel.pending);
        panel.shutdown();
    }

    #[test]
    fn invalid_context_is_explicit_and_bounded() -> TestResult {
        let root = tempfile::tempdir()?;
        let mut storage = Storage::default();
        assert!(
            storage
                .prepare(Context::Attach(vec![root.path().to_path_buf()]))
                .is_err()
        );
        assert!(
            storage
                .prepare(Context::Attach(vec![root.path().join("missing")]))
                .is_err()
        );
        assert!(storage.prepare(Context::Files(Vec::new())).is_err());
        assert!(storage.path("txt", MAX_FILE + 1).is_err());
        storage.bytes = MAX_CONTEXT;
        assert!(storage.path("png", 1).is_err());
        let mut buffer = TextBuffer::load_from_str(BufferId(92), "\x1b[201~\ncommand");
        buffer.set_cursors(vec![Selection {
            anchor: buffer.position_at(0)?,
            head: buffer.position_at(buffer.rope().len_bytes())?,
        }])?;
        assert!(selection_context(root.path(), &buffer.snapshot()).is_err());
        let buffer = TextBuffer::load_from_str(BufferId(93), "no selection");
        assert!(selection_context(root.path(), &buffer.snapshot()).is_err());
        Ok(())
    }
}
