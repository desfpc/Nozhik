//! Main-thread inline completion and model-management adapters.
use nozhik_ai_completion::{DEFAULT_ENDPOINT, Generation, Ollama, PullProgress};
use nozhik_buffer::TextBuffer;
use std::time::{Duration, Instant};

#[derive(Default)]
pub struct Inline {
    pub runtime: Option<tokio::runtime::Handle>,
    pub pending_ghost_text: Option<String>,
    settings: Option<(bool, String, u16)>,
    identity: Option<(nozhik_core::BufferId, u64, usize)>,
    job: Option<Generation>,
    client: Option<Ollama>,
    focused: bool,
    attempted: bool,
    failures: u8,
    retry_at: Option<Instant>,
    pub status: Option<String>,
}
impl Inline {
    #[cfg(test)]
    pub fn at_endpoint(endpoint: &str) -> Result<Self, nozhik_ai_completion::Error> {
        Ok(Self {
            client: Some(Ollama::new(endpoint)?),
            ..Self::default()
        })
    }
    pub fn configure(&mut self, config: &nozhik_config::Config) {
        let settings = (
            config.ai_enabled,
            config.ai_model.clone(),
            config.ai_debounce_ms,
        );
        if self.settings.as_ref() != Some(&settings) {
            self.cancel();
            self.settings = Some(settings);
            self.focused = false;
        }
    }
    pub fn cancel(&mut self) {
        self.job = None;
        self.pending_ghost_text = None;
        self.attempted = true;
        self.retry_at = None;
        self.status = None;
    }
    pub fn rearm(&mut self) {
        self.cancel();
        self.focused = false;
    }
    pub fn synchronize(&mut self, buffer: &TextBuffer, ctx: &egui::Context, focused: bool) {
        let snapshot = buffer.snapshot();
        let cursors = buffer.cursors().as_slice();
        let cursor = &cursors[0];
        let identity = (
            snapshot.buffer_id,
            snapshot.revision,
            cursor.head.byte_offset,
        );
        if !focused || cursors.len() != 1 || cursor.anchor != cursor.head {
            self.rearm();
            self.identity = Some(identity);
            return;
        }
        if self.identity != Some(identity) || !self.focused {
            self.cancel();
            self.identity = Some(identity);
            self.focused = true;
            self.attempted = false;
            self.failures = 0;
        }
        if let Some(result) = self.job.as_mut().and_then(Generation::poll) {
            self.job = None;
            match result {
                Ok(text) => {
                    self.pending_ghost_text = text;
                    self.status = Some(
                        if self.pending_ghost_text.is_some() {
                            "AI: Tab to accept"
                        } else {
                            "AI: no suggestion here"
                        }
                        .into(),
                    );
                }
                Err(error) => {
                    self.status = Some(format!("AI: {error}"));
                    self.failures += 1;
                    // A finite retry handles service startup/model loading without idle polling forever.
                    if self.failures < 3 {
                        self.retry_at = Some(
                            Instant::now() + Duration::from_secs(2 * u64::from(self.failures)),
                        );
                        self.attempted = false;
                    }
                }
            }
            ctx.request_repaint();
        }
        if self.attempted {
            return;
        }
        if let Some(deadline) = self.retry_at {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if !remaining.is_zero() {
                ctx.request_repaint_after(remaining);
                return;
            }
        }
        if let Some((true, model, delay)) = &self.settings
            && let Some(runtime) = &self.runtime
        {
            if self.client.is_none() {
                self.client = Ollama::new(DEFAULT_ENDPOINT).ok();
            }
            let Some(client) = self.client.clone() else {
                return;
            };
            let at = buffer.rope().byte_to_char(cursor.head.byte_offset);
            let prefix = buffer.rope().slice(at.saturating_sub(2000)..at).to_string();
            let suffix = buffer
                .rope()
                .slice(at..(at + 1000).min(buffer.rope().len_chars()))
                .to_string();
            let wake = ctx.clone();
            self.job = Some(Generation::start(
                runtime,
                client,
                model.clone(),
                nozhik_ai_completion::fim::prompt(&prefix, &suffix),
                Duration::from_millis(u64::from(*delay)),
                move || wake.request_repaint(),
            ));
            self.attempted = true;
            self.status = Some("AI: generating…".into());
            ctx.request_repaint();
        }
    }
}

#[derive(Clone, Default)]
enum ModelState {
    #[default]
    Unknown,
    Offline,
    Connected(Vec<String>),
    Pulling(PullProgress),
    Failed(String),
}
#[derive(Default)]
pub struct Models {
    pub runtime: Option<tokio::runtime::Handle>,
    model: String,
    state: ModelState,
    task: Option<tokio::task::JoinHandle<()>>,
    updates: Option<tokio::sync::watch::Receiver<ModelState>>,
    pulling: bool,
}
impl Drop for Models {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}
impl Models {
    pub fn stop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
    fn start(&mut self, ctx: &egui::Context, pull: bool) {
        let Some(runtime) = &self.runtime else {
            return;
        };
        if let Some(task) = self.task.take() {
            task.abort();
        }
        let (sender, receiver) = tokio::sync::watch::channel(ModelState::Unknown);
        self.updates = Some(receiver);
        let model = self.model.clone();
        let wake = ctx.clone();
        self.pulling = pull;
        self.state = if pull {
            ModelState::Pulling(PullProgress::default())
        } else {
            ModelState::Unknown
        };
        self.task = Some(runtime.spawn(async move {
            let state = match Ollama::new(DEFAULT_ENDPOINT) {
                Err(error) => ModelState::Failed(error.to_string()),
                Ok(client) => {
                    if pull {
                        let result = client
                            .pull_model(&model, |progress| {
                                sender.send_replace(ModelState::Pulling(progress));
                                wake.request_repaint();
                            })
                            .await;
                        if let Err(error) = result {
                            sender.send_replace(ModelState::Failed(error.to_string()));
                            wake.request_repaint();
                            return;
                        }
                    }
                    client
                        .models()
                        .await
                        .map_or(ModelState::Offline, ModelState::Connected)
                }
            };
            sender.send_replace(state);
            wake.request_repaint();
        }));
    }
    pub fn show(&mut self, ui: &mut egui::Ui, config: &mut nozhik_config::Config) {
        ui.heading("AI Assistant (Ollama)");
        if let Some(updates) = &mut self.updates {
            self.state = updates.borrow_and_update().clone();
            if !matches!(self.state, ModelState::Unknown | ModelState::Pulling(_)) {
                self.pulling = false;
            }
        }
        ui.checkbox(
            &mut config.ai_enabled,
            "Enable AI Inline Completion (Ghost Text)",
        );
        ui.horizontal(|ui| {
            ui.label("Debounce Delay (ms)");
            ui.add(egui::DragValue::new(&mut config.ai_debounce_ms).range(50..=5000));
        });
        ui.label("Model name");
        ui.add_enabled(
            !self.pulling,
            egui::TextEdit::singleline(&mut config.ai_model),
        );
        if self.model != config.ai_model {
            self.model.clone_from(&config.ai_model);
            self.start(ui.ctx(), false);
        }
        match &self.state {
            ModelState::Offline => {
                ui.weak("● Offline");
                ui.label("Ollama unavailable. Install Ollama from ollama.com; Nozhik starts it automatically when AI is enabled.");
                ui.hyperlink_to("Get Ollama", "https://ollama.com");
            }
            ModelState::Unknown => {
                ui.weak("Checking service…");
            }
            ModelState::Failed(error) => {
                ui.colored_label(crate::theme::ERROR, error);
            }
            ModelState::Connected(models) => {
                ui.colored_label(egui::Color32::from_rgb(90, 180, 100), "● Connected");
                let canonical = |name: &str| {
                    if name.contains(':') {
                        name.to_owned()
                    } else {
                        format!("{name}:latest")
                    }
                };
                if models
                    .iter()
                    .any(|m| canonical(m) == canonical(&self.model))
                {
                    egui::Frame::new()
                        .fill(egui::Color32::from_rgb(35, 65, 100))
                        .corner_radius(4)
                        .inner_margin(6)
                        .show(ui, |ui| {
                            ui.label("Model Installed & Ready");
                        });
                } else if ui
                    .button(format!("Download Model ({})", self.model))
                    .clicked()
                {
                    self.start(ui.ctx(), true);
                }
            }
            ModelState::Pulling(progress) => {
                ui.add_enabled(false, egui::Button::new("Downloading Model…"));
                ui.label(&progress.status);
                #[allow(
                    clippy::cast_precision_loss,
                    reason = "Human-readable download byte counts"
                )]
                let label = format!(
                    "{:.1} MB / {:.1} MB ({:.0}%)",
                    progress.completed as f64 / 1_000_000.0,
                    progress.total as f64 / 1_000_000.0,
                    progress.fraction() * 100.0
                );
                ui.add(egui::ProgressBar::new(progress.fraction()).text(label));
            }
        }
        if !self.pulling && ui.button("Refresh service status").clicked() {
            self.start(ui.ctx(), false);
        }
        ui.weak("Local endpoint: http://127.0.0.1:11434 · Tab to accept · Esc to dismiss");
    }
}
