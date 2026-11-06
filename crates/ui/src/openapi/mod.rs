//! `OpenAPI` JSON documents open as normal JSON, with a session JSON/GUI switch.
mod http;
mod spec;
mod view;

use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::mpsc,
};

const MAX_SPEC: usize = 4 * 1024 * 1024;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Json,
    Gui,
}

pub enum Parsed {
    Spec(Box<spec::Spec>),
    Invalid(String),
}

#[derive(Clone)]
pub struct Draft {
    pub params: HashMap<String, String>,
    pub content: usize,
    pub body: String,
    pub server: usize,
    pub vars: Vec<String>,
}

pub struct Pending {
    pub rx: mpsc::Receiver<http::Outcome>,
}

pub struct FileState {
    revision: u64,
    mode: Mode,
    pub parsed: Parsed,
    pub server: usize,
    pub vars: Vec<String>,
    pub vars_server: usize,
    pub auth_open: bool,
    pub auth: HashMap<String, spec::AuthInput>,
    pub collapsed_tags: HashSet<String>,
    pub open_ops: HashSet<String>,
    pub schemas_open: bool,
    pub open_schemas: HashSet<String>,
    pub schema_nodes: HashSet<String>,
    pub trying: HashSet<String>,
    pub drafts: HashMap<String, Draft>,
    pub schema_tab: HashSet<String>,
    pub live: HashMap<String, http::Outcome>,
    pub pending: HashMap<String, Pending>,
    schemas_seeded: bool,
}

impl Default for FileState {
    fn default() -> Self {
        Self {
            revision: u64::MAX,
            mode: Mode::Json,
            parsed: Parsed::Invalid(String::new()),
            server: 0,
            vars: Vec::new(),
            vars_server: usize::MAX,
            auth_open: false,
            auth: HashMap::new(),
            collapsed_tags: HashSet::new(),
            open_ops: HashSet::new(),
            schemas_open: true,
            open_schemas: HashSet::new(),
            schema_nodes: HashSet::new(),
            trying: HashSet::new(),
            drafts: HashMap::new(),
            schema_tab: HashSet::new(),
            live: HashMap::new(),
            pending: HashMap::new(),
            schemas_seeded: false,
        }
    }
}

enum Class {
    Foreign,
    Invalid(String),
    Spec(Box<spec::Spec>),
}

#[derive(Default)]
pub struct OpenApiUi {
    files: HashMap<PathBuf, FileState>,
    http: http::Hub,
}

impl OpenApiUi {
    fn reload(&mut self, path: &Path, revision: u64, text: &str) {
        let mode = self.files.get(path).map_or(Mode::Json, |state| state.mode);
        let previous = self.files.remove(path);
        match classify(text) {
            Class::Foreign => {}
            Class::Invalid(error) => {
                let mut state = previous.unwrap_or_default();
                state.revision = revision;
                state.mode = mode;
                state.parsed = Parsed::Invalid(error);
                self.files.insert(path.to_path_buf(), state);
            }
            Class::Spec(spec) => {
                let mut state = previous.unwrap_or_default();
                state.revision = revision;
                state.mode = mode;
                if state.server >= spec.servers.len() {
                    state.server = 0;
                }
                seed_schemas(&mut state, &spec);
                state.parsed = Parsed::Spec(spec);
                self.files.insert(path.to_path_buf(), state);
            }
        }
    }

    fn poll(&mut self, path: &Path) {
        let Some(state) = self.files.get_mut(path) else {
            return;
        };
        let keys: Vec<String> = state.pending.keys().cloned().collect();
        let mut finished = Vec::new();
        for key in keys {
            let Some(pending) = state.pending.get(&key) else {
                continue;
            };
            match pending.rx.try_recv() {
                Ok(outcome) => finished.push((key, outcome)),
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    finished.push((
                        key,
                        http::Outcome::transport("HTTP worker stopped", String::new()),
                    ));
                }
            }
        }
        for (key, outcome) in finished {
            state.pending.remove(&key);
            state.live.insert(key, outcome);
        }
    }
}

fn seed_schemas(state: &mut FileState, spec: &spec::Spec) {
    if state.schemas_seeded {
        return;
    }
    if spec.schemas.len() <= 12 {
        for schema in &spec.schemas {
            state.open_schemas.insert(schema.name.clone());
        }
    }
    state.schemas_seeded = true;
}

fn classify(text: &str) -> Class {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    if text.len() > MAX_SPEC {
        return if mentions(text) {
            Class::Invalid("OpenAPI document is larger than 4 MiB".into())
        } else {
            Class::Foreign
        };
    }
    let value = match serde_json::from_str::<Value>(text) {
        Ok(value) => value,
        Err(error) => {
            return if mentions(text) {
                Class::Invalid(error.to_string())
            } else {
                Class::Foreign
            };
        }
    };
    let Some(object) = value.as_object() else {
        return Class::Foreign;
    };
    let openapi = object.get("openapi").and_then(Value::as_str);
    let swagger = object.get("swagger").and_then(Value::as_str);
    if openapi.is_some_and(|version| version.starts_with('3')) || swagger == Some("2.0") {
        return match spec::parse_document(&value) {
            Ok(document) => Class::Spec(Box::new(document)),
            Err(error) => Class::Invalid(error),
        };
    }
    if openapi.is_some() || swagger.is_some() {
        Class::Invalid("Only OpenAPI 3.x and Swagger 2.0 JSON documents are supported".into())
    } else {
        Class::Foreign
    }
}

fn mentions(text: &str) -> bool {
    let mut end = text.len();
    for (count, (index, _)) in text.char_indices().enumerate() {
        if count == 8_000 {
            end = index;
            break;
        }
    }
    let head = text[..end].to_ascii_lowercase();
    head.contains("\"openapi\"") || head.contains("\"swagger\"")
}

fn candidate(path: &Path, known: bool) -> bool {
    known
        || crate::project_icons::is_openapi_filename(path)
        || path
            .extension()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case("json"))
}

impl crate::NozhikUi {
    /// Draws the JSON/GUI switch for an `OpenAPI` document.
    ///
    /// Returns whether the GUI replaced the text editor for this frame.
    pub fn openapi_toolbar(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &egui::Context,
        index: usize,
        enabled: bool,
    ) -> bool {
        let path = self.documents[index].path.clone();
        let revision = self.documents[index].buffer.snapshot().revision;
        let json = nozhik_config::languages::language_for_path(&path)
            == Some(nozhik_config::languages::LanguageId::Json);
        if candidate(&path, self.openapi.files.contains_key(&path))
            && self
                .openapi
                .files
                .get(&path)
                .is_none_or(|state| state.revision != revision)
        {
            let text = self.documents[index].buffer.rope().to_string();
            self.openapi.reload(&path, revision, &text);
        }
        self.openapi.poll(&path);
        let document = self.openapi.files.contains_key(&path);
        let gui = self
            .openapi
            .files
            .get(&path)
            .is_some_and(|state| state.mode == Mode::Gui);
        if document || (json && !gui) {
            ui.horizontal(|ui| {
                if let Some(state) = self.openapi.files.get_mut(&path) {
                    ui.selectable_value(&mut state.mode, Mode::Json, "JSON");
                    ui.selectable_value(&mut state.mode, Mode::Gui, "GUI");
                    ui.add_space(8.0);
                }
                if json && !gui && enabled {
                    if ui.button("Open in Tools / JSON").clicked() {
                        let text = self.documents[index].buffer.rope().to_string();
                        self.tools.load(text);
                        self.left = Some(crate::LeftTool::Tools);
                    }
                    if ui.button("Reformat JSON").clicked() {
                        self.reformat_code(ctx);
                    }
                }
            });
        }
        if !gui {
            return false;
        }
        let OpenApiUi { files, http } = &mut self.openapi;
        if let Some(state) = files.get_mut(&path) {
            view::show(ui, ctx, &path, state, http, enabled);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Document, NozhikUi};
    use egui::{Event, PointerButton, Pos2, Rect, vec2};

    const FIXTURE: &str = r#"{
      "openapi": "3.0.3",
      "info": {"title": "nozhove.com API", "version": "1.0.0", "description": "HTTP API for nozhove.com"},
      "servers": [{"url": "http://localhost:8181", "description": "Local Docker"}],
      "paths": {"/api/v1/profile/auth": {"post": {
        "summary": "Login with email and password",
        "responses": {"200": {"description": "JWT tokens"}}
      }}}
    }"#;

    #[test]
    fn ordinary_json_has_no_switch_and_broken_openapi_keeps_one() {
        assert!(matches!(classify(r#"{"name":"IceCMS"}"#), Class::Foreign));
        assert!(matches!(classify(FIXTURE), Class::Spec(_)));
        let mut ui = OpenApiUi::default();
        let path = PathBuf::from("openapi.json");
        ui.reload(&path, 1, FIXTURE);
        assert!(
            ui.files
                .get(&path)
                .is_some_and(|state| state.mode == Mode::Json)
        );
        if let Some(state) = ui.files.get_mut(&path) {
            state.mode = Mode::Gui;
        }
        ui.reload(&path, 2, "{\"openapi\": ");
        assert!(matches!(
            ui.files.get(&path).map(|state| &state.parsed),
            Some(Parsed::Invalid(_))
        ));
        assert!(
            ui.files
                .get(&path)
                .is_some_and(|state| state.mode == Mode::Gui)
        );
        ui.reload(&path, 3, r#"{"name":"notes"}"#);
        assert!(!ui.files.contains_key(&path));
    }

    #[test]
    fn editor_opens_json_until_the_gui_switch() -> Result<(), String> {
        let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
        let path = dir.path().join("openapi.json");
        let bus = nozhik_event_bus::EventBus::new(32).map_err(|error| error.to_string())?;
        let settings =
            nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())
                .map_err(|error| error.to_string())?;
        let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
        shell.documents.push(Document {
            path,
            binary: false,
            name: "openapi.json".into(),
            open: true,
            buffer: nozhik_buffer::TextBuffer::load_from_str(nozhik_core::BufferId(7), FIXTURE),
        });
        shell.active = Some(0);
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        let first = frame(&mut shell, &ctx, Vec::new());
        assert!(galley(&first, "GUI").is_some());
        assert!(!contains(&first, "Authorize"));
        assert!(contains(&first, "Open in Tools / JSON"));
        let gui = galley(&first, "GUI").ok_or_else(|| "GUI switch is missing".to_string())?;
        let _ = frame(&mut shell, &ctx, click(gui.center()));
        let second = frame(&mut shell, &ctx, Vec::new());
        assert!(contains(&second, "Authorize"), "GUI should list Authorize");
        assert!(contains(&second, "POST"));
        assert!(contains(&second, "/api/v1/profile/auth"));
        assert!(contains(&second, "nozhove.com API"));
        assert!(!contains(&second, "Open in Tools / JSON"));
        Ok(())
    }

    fn frame(shell: &mut NozhikUi, ctx: &egui::Context, events: Vec<Event>) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1100.0, 800.0))),
                events,
                ..Default::default()
            },
            |ctx| shell.editor_area(ctx),
        )
    }

    fn click(pos: Pos2) -> Vec<Event> {
        vec![
            Event::PointerMoved(pos),
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    fn galley(output: &egui::FullOutput, label: &str) -> Option<Rect> {
        output.shapes.iter().find_map(|shape| match &shape.shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                Some(Rect::from_min_size(text.pos, text.galley.size()))
            }
            _ => None,
        })
    }

    fn contains(output: &egui::FullOutput, label: &str) -> bool {
        output.shapes.iter().any(|shape| match &shape.shape {
            egui::Shape::Text(text) => text.galley.text().contains(label),
            _ => false,
        })
    }
}
