//! Cancellable commit-message generation with index and form revision gates.
use super::{Task, VcsPanel};
use nozhik_ai_completion::{Ollama, ollama::CommitMessage};
use nozhik_vcs::model::CommitContext;
use tokio::{runtime::Handle, sync::oneshot, task::JoinHandle};

#[derive(Default)]
pub struct CommitAi {
    pub runtime: Option<Handle>,
    enabled: bool,
    model: String,
    pub(super) client: Option<Ollama>,
    pending: Option<Pending>,
}
struct Pending {
    request: u64,
    original: (String, String),
    context: Option<CommitContext>,
    result: Option<oneshot::Receiver<Result<CommitMessage, String>>>,
    task: Option<JoinHandle<()>>,
    message: Option<CommitMessage>,
}
impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}
impl CommitAi {
    pub fn cancel(&mut self) {
        self.pending = None;
    }
    pub(super) const fn busy(&self) -> bool {
        self.pending.is_some()
    }
}
impl VcsPanel {
    pub(crate) fn configure_commit_ai(&mut self, config: &nozhik_config::Config) {
        if self.commit_ai.enabled != config.ai_enabled || self.commit_ai.model != config.ai_model {
            self.commit_ai.cancel();
            self.commit_ai.enabled = config.ai_enabled;
            self.commit_ai.model.clone_from(&config.ai_model);
        }
    }
    pub(super) fn commit_ai_button(&mut self, ui: &mut egui::Ui, staged: usize) {
        let busy = self.commit_ai.busy();
        let available = self.commit_ai.enabled
            && !self.commit_ai.model.trim().is_empty()
            && self.commit_ai.runtime.is_some()
            && staged > 0
            && self.mutations.is_empty()
            && self
                .repo
                .as_ref()
                .is_some_and(|repo| repo.operation.is_none() && repo.conflicts.is_empty());
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    available && !busy,
                    egui::Button::new(if busy {
                        "Generating…"
                    } else {
                        "Auto-fill Commit Message"
                    }),
                )
                .on_disabled_hover_text(
                    "Enable AI completion and select at least one file for the commit",
                )
                .clicked()
            {
                self.error = None;
                let request = self.submit(Task::CommitContext);
                self.commit_ai.pending = Some(Pending {
                    request,
                    original: (self.summary.clone(), self.body.clone()),
                    context: None,
                    result: None,
                    task: None,
                    message: None,
                });
            }
            if busy && ui.button("Cancel").clicked() {
                self.commit_ai.cancel();
            }
        });
    }
    pub(super) fn tick_commit_ai(&mut self) {
        let Some(mut pending) = self.commit_ai.pending.take() else {
            return;
        };
        if pending.original != (self.summary.clone(), self.body.clone()) {
            return;
        }
        if let Some(receiver) = &mut pending.result {
            match receiver.try_recv() {
                Ok(Ok(message)) => {
                    pending.message = Some(message);
                    pending.result = None;
                    // Re-read the exact index after generation, including externally staged changes.
                    pending.request = self.submit(Task::CommitContext);
                }
                Ok(Err(error)) => {
                    self.error = Some(format!("AI commit message: {error}"));
                    return;
                }
                Err(oneshot::error::TryRecvError::Closed) => {
                    self.error = Some("AI commit message task stopped".into());
                    return;
                }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        self.commit_ai.pending = Some(pending);
    }
    pub(super) fn accept_commit_context(
        &mut self,
        id: u64,
        context: CommitContext,
        ctx: &egui::Context,
    ) {
        let Some(mut pending) = self.commit_ai.pending.take() else {
            return;
        };
        if pending.request != id {
            self.commit_ai.pending = Some(pending);
            return;
        }
        if pending.original != (self.summary.clone(), self.body.clone()) {
            return;
        }
        if let Some(message) = pending.message.take() {
            if pending
                .context
                .as_ref()
                .is_some_and(|old| old.fingerprint == context.fingerprint)
            {
                self.summary = message.summary;
                self.body = message.description;
            } else {
                self.error = Some(
                    "Staged changes changed during generation. Generate the commit message again."
                        .into(),
                );
            }
            return;
        }
        let Some(runtime) = &self.commit_ai.runtime else {
            return;
        };
        if self.commit_ai.client.is_none() {
            match Ollama::new(nozhik_ai_completion::DEFAULT_ENDPOINT) {
                Ok(client) => self.commit_ai.client = Some(client),
                Err(error) => {
                    self.error = Some(error.to_string());
                    return;
                }
            }
        }
        let Some(client) = self.commit_ai.client.clone() else {
            return;
        };
        let model = self.commit_ai.model.clone();
        let patch = context.patch.clone();
        pending.context = Some(context);
        let (sender, receiver) = oneshot::channel();
        let wake = ctx.clone();
        pending.task = Some(runtime.spawn(async move {
            let result = client
                .commit_message(&model, &patch)
                .await
                .map_err(|error| error.to_string());
            // A cancelled or closed form intentionally discards this result.
            let _ = sender.send(result);
            wake.request_repaint();
        }));
        pending.result = Some(receiver);
        self.commit_ai.pending = Some(pending);
    }
    pub(super) fn commit_context_failed(&mut self, id: u64) {
        if self
            .commit_ai
            .pending
            .as_ref()
            .is_some_and(|pending| pending.request == id)
        {
            self.commit_ai.cancel();
        }
    }
}
