//! Project SFTP deployment settings, explicit requests and transfer progress.
use nozhik_config::{Config, deployment::SftpDeployment};
use nozhik_remote_ssh::{
    SshProfile, SshSession,
    credentials::same_identity,
    deployment::{DeploymentPlan, DeploymentProgress},
    sftp::SftpClient,
};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Default)]
pub struct DeploymentSettings {
    source: Option<SftpDeployment>,
    profile: String,
    root: String,
    error: String,
    missing_profile: bool,
}
impl DeploymentSettings {
    /// Settings failure, including a selected SSH profile that disappeared.
    pub(crate) fn problem(&self) -> Option<String> {
        if !self.error.is_empty() {
            return Some(self.error.clone());
        }
        self.missing_profile.then(|| {
            "The selected SSH connection no longer exists. Choose another connection.".to_owned()
        })
    }
}
impl DeploymentSettings {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        config: &mut Config,
        profiles: Option<&[SshProfile]>,
    ) {
        ui.heading("SFTP Deployment");
        let Some(profiles) = profiles else {
            self.missing_profile = false;
            ui.weak("Open a project to configure its deployment server.");
            return;
        };
        if self.source != config.sftp_deployment {
            self.source.clone_from(&config.sftp_deployment);
            self.profile = self
                .source
                .as_ref()
                .map_or_else(String::new, |s| s.ssh_profile.clone());
            self.root = self
                .source
                .as_ref()
                .map_or_else(String::new, |s| s.remote_root.clone());
        }
        ui.label("One deployment server per project. SSH credentials and host verification use the saved connection.");
        if profiles.is_empty() {
            ui.weak("Create an SSH connection in Remote Host / SSH first.");
        }
        ui.label("SSH connection");
        egui::ComboBox::from_id_salt("deployment_ssh_profile")
            .selected_text(if self.profile.is_empty() {
                "Choose connection…"
            } else {
                &self.profile
            })
            .show_ui(ui, |ui| {
                for profile in profiles {
                    ui.selectable_value(&mut self.profile, profile.name.clone(), &profile.name);
                }
            });
        self.missing_profile =
            !self.profile.is_empty() && !profiles.iter().any(|p| p.name == self.profile);
        ui.label("Remote project root");
        ui.add(
            egui::TextEdit::singleline(&mut self.root)
                .hint_text("/var/www/project")
                .desired_width(f32::INFINITY),
        );
        ui.weak("Project-relative paths are preserved. Deploy uploads saved files and overwrites matching remote files; unrelated remote files are kept.");
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    profiles.iter().any(|p| p.name == self.profile),
                    egui::Button::new("Apply deployment"),
                )
                .clicked()
            {
                let root = nozhik_remote_ssh::deployment::remote_root(self.root.trim());
                match root {
                    Ok(remote_root) => {
                        let selected = SftpDeployment {
                            ssh_profile: self.profile.clone(),
                            remote_root,
                        };
                        match selected.validate() {
                            Ok(()) => {
                                config.sftp_deployment = Some(selected);
                                self.error.clear();
                            }
                            Err(e) => self.error = e.to_string(),
                        }
                    }
                    Err(e) => self.error = e.to_string(),
                }
            }
            if ui
                .add_enabled(
                    config.sftp_deployment.is_some(),
                    egui::Button::new("Disable deployment"),
                )
                .clicked()
            {
                config.sftp_deployment = None;
                self.profile.clear();
                self.root.clear();
                self.error.clear();
            }
        });
    }
}
enum Sources {
    Path(PathBuf),
    Files(Vec<PathBuf>),
}
impl Sources {
    fn includes(&self, path: &std::path::Path) -> bool {
        match self {
            Self::Path(root) => path.starts_with(root),
            Self::Files(files) => files.iter().any(|p| p == path),
        }
    }
}
struct Pending {
    id: u64,
    config: SftpDeployment,
    profile: SshProfile,
    project: PathBuf,
    sources: Sources,
}
#[derive(Default)]
struct TransferState {
    progress: DeploymentProgress,
    finished: Option<Result<DeploymentProgress, String>>,
}
struct Transfer {
    cancelled: Arc<AtomicBool>,
    state: Arc<Mutex<TransferState>>,
    done: std::sync::mpsc::Receiver<()>,
}
impl Transfer {
    fn start(pending: Pending, session: SshSession, ctx: egui::Context) -> Self {
        let cancelled = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(TransferState::default()));
        let (sender, done) = std::sync::mpsc::channel();
        let worker_cancel = cancelled.clone();
        let worker_state = state.clone();
        std::thread::spawn(move || {
            let execute = || -> Result<DeploymentProgress, String> {
                let plan = match &pending.sources {
                    Sources::Path(selection) => {
                        DeploymentPlan::build(&pending.project, selection, &worker_cancel)
                    }
                    Sources::Files(paths) => {
                        DeploymentPlan::files(&pending.project, paths, &worker_cancel)
                    }
                }
                .map_err(|e| e.to_string())?;
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .map_err(|e| e.to_string())?;
                let mut last_update = Instant::now();
                let result = runtime.block_on(async {
                    let transfer = async {
                        let mut client = SftpClient::new(&session).await?;
                        client.deploy(&plan,&pending.config.remote_root,|progress| {
                            if let Ok(mut state) = worker_state.lock() { state.progress=progress; }
                            if last_update.elapsed() >= Duration::from_millis(100) { ctx.request_repaint(); last_update=Instant::now(); }
                        }).await
                    };
                    let cancelled = async {
                        loop { if worker_cancel.load(Ordering::Acquire) { break; } tokio::time::sleep(Duration::from_millis(50)).await; }
                    };
                    tokio::select! {
                        result = transfer => result.map_err(|e| e.to_string()),
                        () = cancelled => Err("SFTP deployment cancelled; files already sent are retained and the current file may be partial".into()),
                    }
                });
                runtime.shutdown_timeout(Duration::from_secs(2));
                result
            };
            let result = execute();
            if let Ok(mut state) = worker_state.lock() {
                state.finished = Some(result);
            }
            ctx.request_repaint();
            let _sent = sender.send(());
        });
        Self {
            cancelled,
            state,
            done,
        }
    }
}
impl Drop for Transfer {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
#[derive(Default)]
pub struct DeploymentUi {
    pending: Option<Pending>,
    transfer: Option<Transfer>,
    next_id: u64,
    message: String,
}
impl DeploymentUi {
    /// Progress UI stays hidden once the only remaining text is a recorded failure.
    pub fn visible(&self) -> bool {
        self.busy() || (!self.message.is_empty() && self.failure_message().is_none())
    }

    /// Idle message that should be a session problem rather than a progress line.
    pub(crate) fn failure_message(&self) -> Option<(&str, crate::problems::Severity)> {
        let message = self.message.as_str();
        if message.is_empty()
            || message.starts_with("Connecting")
            || message.starts_with("SFTP Deploy complete")
            || message.starts_with("SFTP Deploy →")
            || message.starts_with("SFTP Deploy: connecting")
            || message.starts_with("Cancelling")
        {
            return None;
        }
        let severity = if message.contains("stopped") || message.contains("exhausted") {
            crate::problems::Severity::Critical
        } else if message.contains("cancelled") {
            crate::problems::Severity::Warning
        } else {
            crate::problems::Severity::Error
        };
        Some((message, severity))
    }

    pub(crate) fn incidents(&self) -> Vec<crate::problems::Incident> {
        self.failure_message()
            .map_or_else(Vec::new, |(message, severity)| {
                vec![crate::problems::incident(
                    "sftp-deploy",
                    severity,
                    "SFTP deployment",
                    message,
                )]
            })
    }
    pub fn accepts(&self, id: u64) -> bool {
        self.pending.as_ref().is_some_and(|p| p.id == id)
    }
    pub const fn busy(&self) -> bool {
        self.pending.is_some() || self.transfer.is_some()
    }
    pub fn cancel(&mut self, id: Option<u64>, message: &str) {
        if id.is_none() || self.pending.as_ref().is_some_and(|p| Some(p.id) == id) {
            self.pending = None;
            if let Some(transfer) = &self.transfer {
                transfer.cancelled.store(true, Ordering::Release);
            }
            self.message = message.into();
        }
    }
    pub fn shutdown(&mut self) {
        self.pending = None;
        if let Some(transfer) = self.transfer.take() {
            transfer.cancelled.store(true, Ordering::Release);
            let _finished = transfer.done.recv_timeout(Duration::from_secs(3));
        }
    }
    pub fn tick(&mut self) -> Option<String> {
        let result = self
            .transfer
            .as_ref()
            .and_then(|t| t.state.try_lock().ok()?.finished.clone());
        if let Some(result) = result {
            self.transfer = None;
            self.message = match result {
                Ok(p) => format!(
                    "SFTP Deploy complete: {} files, {} bytes; {} paths skipped",
                    p.files_done, p.bytes_done, p.skipped
                ),
                Err(error) => format!("SFTP Deploy: {error}"),
            };
            Some(self.message.clone())
        } else {
            None
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui) {
        if !self.busy() && self.message.is_empty() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if self.busy() {
                ui.spinner();
            }
            let failure = self.failure_message().is_some();
            ui.label(if self.pending.is_some() {
                "SFTP Deploy: waiting for SSH connection…"
            } else if failure {
                "SFTP Deploy"
            } else {
                self.message.as_str()
            });
            if self.busy() && ui.button("Cancel deployment").clicked() {
                let message = if self.transfer.is_some() {
                    "Cancelling SFTP deployment…"
                } else {
                    "SFTP deployment cancelled"
                };
                self.cancel(None, message);
            }
        });
        if let Some(transfer) = &self.transfer
            && let Ok(state) = transfer.state.try_lock()
        {
            ui.small(format!(
                "{} / {} files · {} / {} bytes · {}",
                state.progress.files_done,
                state.progress.files_total,
                state.progress.bytes_done,
                state.progress.bytes_total,
                state.progress.current
            ));
        }
        ui.separator();
    }
}
impl Drop for DeploymentUi {
    fn drop(&mut self) {
        self.shutdown();
    }
}
impl crate::NozhikUi {
    pub(crate) fn deploy_path(&mut self, selection: PathBuf) {
        self.start_deployment(Sources::Path(selection));
    }
    pub(crate) fn deploy_files(&mut self, paths: Vec<PathBuf>) {
        self.start_deployment(Sources::Files(paths));
    }
    fn start_deployment(&mut self, sources: Sources) {
        let result = (|| -> Result<(), String> {
            if self.deployment.busy() {
                return Err("An SFTP deployment is already in progress".into());
            }
            let config = self
                .config
                .sftp_deployment
                .clone()
                .ok_or("Configure SFTP Deployment in project settings first")?;
            config.validate().map_err(|e| e.to_string())?;
            let project = self
                .project
                .as_ref()
                .ok_or("Open a project first")?
                .root_path
                .clone();
            if matches!(&sources, Sources::Path(selection) if !selection.starts_with(&project)) {
                return Err("Deployment selection is outside this project".into());
            }
            self.deployment_sources_saved(&sources)?;
            let profile=self.remote.database_profiles().iter().find(|p| p.name==config.ssh_profile).cloned().ok_or("Deployment SSH connection no longer exists; choose another in project settings")?;
            let id = self
                .deployment
                .next_id
                .checked_add(1)
                .ok_or("Deployment IDs exhausted")?;
            self.deployment.next_id = id;
            self.deployment.pending = Some(Pending {
                id,
                config,
                profile: profile.clone(),
                project,
                sources,
            });
            if !self
                .remote
                .connect(profile, crate::remote_worker::Intent::Deployment(id))
            {
                self.deployment.pending = None;
                return Err("SSH queue full/stopped".into());
            }
            self.deployment.message = "Connecting for SFTP deployment…".into();
            Ok(())
        })();
        match result {
            Ok(()) => self.status = "SFTP Deploy: connecting…".into(),
            Err(error) => {
                self.deployment.message.clone_from(&error);
                self.status = error;
            }
        }
    }
    fn deployment_sources_saved(&self, sources: &Sources) -> Result<(), String> {
        if self.documents.iter().any(|d| {
            d.open
                && d.buffer.is_dirty()
                && sources.includes(&d.path)
                && !self.remote.documents.contains_key(&d.path)
        }) {
            return Err("Save files in the selected path before SFTP Deploy; unsaved editor changes were not uploaded".into());
        }
        Ok(())
    }
    pub(crate) fn deployment_connected(
        &mut self,
        id: u64,
        session: SshSession,
        ctx: &egui::Context,
    ) {
        if self.deployment.pending.as_ref().is_none_or(|p| p.id != id) {
            return;
        }
        let Some(pending) = self.deployment.pending.take() else {
            return;
        };
        let valid = self.config.sftp_deployment.as_ref() == Some(&pending.config)
            && self
                .project
                .as_ref()
                .is_some_and(|p| p.root_path == pending.project)
            && self
                .remote
                .database_profiles()
                .iter()
                .any(|p| same_identity(p, &pending.profile));
        let saved = self.deployment_sources_saved(&pending.sources);
        if !valid || saved.is_err() {
            self.status = saved.err().unwrap_or_else(|| {
                "Deployment settings or SSH connection changed; start deployment again".into()
            });
            self.deployment.message.clone_from(&self.status);
            return;
        }
        self.deployment.message = format!(
            "SFTP Deploy → {}:{}",
            pending.profile.name, pending.config.remote_root
        );
        self.deployment.transfer = Some(Transfer::start(pending, session, ctx.clone()));
    }
    pub(crate) fn deployment_tick(&mut self) {
        if let Some(message) = self.deployment.tick() {
            self.status = message;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn profile(name: &str) -> SshProfile {
        SshProfile {
            name: name.into(),
            host: "example.invalid".into(),
            port: 22,
            username: "deploy".into(),
            auth: nozhik_remote_ssh::AuthMethod::Agent,
            host_key_sha256: String::new(),
            save_password: false,
            forwards: vec![],
        }
    }
    fn frame(
        settings: &mut DeploymentSettings,
        config: &mut Config,
        profiles: &[SshProfile],
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(650.0, 430.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| settings.show(ui, config, Some(profiles)));
            },
        )
    }
    fn label(output: &egui::FullOutput, text: &str) -> Result<egui::Pos2, String> {
        output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.text() == text => {
                    Some(t.pos + t.galley.size() * 0.5)
                }
                _ => None,
            })
            .ok_or_else(|| format!("Missing deployment UI label: {text}"))
    }
    fn click(
        settings: &mut DeploymentSettings,
        config: &mut Config,
        profiles: &[SshProfile],
        ctx: &egui::Context,
        point: egui::Pos2,
    ) {
        for pressed in [true, false] {
            frame(
                settings,
                config,
                profiles,
                ctx,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
    }
    #[test]
    fn settings_choose_existing_connection_apply_one_target_validate_and_disable()
    -> Result<(), Box<dyn std::error::Error>> {
        let ctx = egui::Context::default();
        ctx.style_mut(|s| s.animation_time = 0.0);
        let profiles = vec![profile("Production"), profile("Staging")];
        let mut settings = DeploymentSettings::default();
        let mut config = Config::default();
        let output = frame(&mut settings, &mut config, &profiles, &ctx, vec![]);
        click(
            &mut settings,
            &mut config,
            &profiles,
            &ctx,
            label(&output, "Choose connection…")?,
        );
        let output = frame(&mut settings, &mut config, &profiles, &ctx, vec![]);
        click(
            &mut settings,
            &mut config,
            &profiles,
            &ctx,
            label(&output, "Production")?,
        );
        let output = frame(&mut settings, &mut config, &profiles, &ctx, vec![]);
        click(
            &mut settings,
            &mut config,
            &profiles,
            &ctx,
            label(&output, "/var/www/project")?,
        );
        frame(
            &mut settings,
            &mut config,
            &profiles,
            &ctx,
            vec![egui::Event::Text("/srv/app/".into())],
        );
        let output = frame(&mut settings, &mut config, &profiles, &ctx, vec![]);
        click(
            &mut settings,
            &mut config,
            &profiles,
            &ctx,
            label(&output, "Apply deployment")?,
        );
        assert_eq!(
            config.sftp_deployment,
            Some(SftpDeployment {
                ssh_profile: "Production".into(),
                remote_root: "/srv/app".into()
            })
        );
        frame(&mut settings, &mut config, &profiles, &ctx, vec![]);
        settings.root = "../bad".into();
        let output = frame(&mut settings, &mut config, &profiles, &ctx, vec![]);
        click(
            &mut settings,
            &mut config,
            &profiles,
            &ctx,
            label(&output, "Apply deployment")?,
        );
        assert_ne!(settings.error, "");
        assert_eq!(
            config
                .sftp_deployment
                .as_ref()
                .map(|d| d.remote_root.as_str()),
            Some("/srv/app")
        );
        settings.profile = "Staging".into();
        settings.root = "/srv/other".into();
        let output = frame(&mut settings, &mut config, &profiles, &ctx, vec![]);
        click(
            &mut settings,
            &mut config,
            &profiles,
            &ctx,
            label(&output, "Apply deployment")?,
        );
        assert_eq!(
            config
                .sftp_deployment
                .as_ref()
                .map(|d| d.ssh_profile.as_str()),
            Some("Staging")
        );
        let output = frame(&mut settings, &mut config, &profiles, &ctx, vec![]);
        click(
            &mut settings,
            &mut config,
            &profiles,
            &ctx,
            label(&output, "Disable deployment")?,
        );
        assert!(config.sftp_deployment.is_none());
        Ok(())
    }
    #[test]
    fn cancellation_ignores_other_request_and_dirty_subtrees_require_save()
    -> Result<(), Box<dyn std::error::Error>> {
        let root = tempfile::tempdir()?;
        let mut deployment = DeploymentUi {
            pending: Some(Pending {
                id: 7,
                config: SftpDeployment {
                    ssh_profile: "Production".into(),
                    remote_root: "/srv/app".into(),
                },
                profile: profile("Production"),
                project: root.path().into(),
                sources: Sources::Path(root.path().into()),
            }),
            transfer: None,
            next_id: 0,
            message: String::new(),
        };
        deployment.cancel(Some(8), "unrelated");
        assert!(deployment.accepts(7));
        deployment.cancel(Some(7), "cancelled");
        assert!(!deployment.busy());
        let bus = nozhik_event_bus::EventBus::new(16)?;
        let manager = nozhik_config::ConfigManager::from_path(
            root.path().join("settings.toml"),
            bus.clone(),
        )?;
        let mut ui = crate::NozhikUi::new(bus, manager, Config::default());
        let mut buffer =
            nozhik_buffer::TextBuffer::load_from_str(nozhik_core::BufferId(1), "saved");
        buffer.insert(0, "unsaved")?;
        ui.documents.push(crate::Document {
            path: root.path().join("src/a.php"),
            binary: false,
            name: "a.php".into(),
            buffer,
            open: true,
        });
        assert!(
            ui.deployment_sources_saved(&Sources::Path(root.path().join("src")))
                .is_err()
        );
        assert!(
            ui.deployment_sources_saved(&Sources::Path(root.path().join("other")))
                .is_ok()
        );
        assert!(
            ui.deployment_sources_saved(&Sources::Files(vec![root.path().join("src/a.php")]))
                .is_err()
        );
        assert!(
            ui.deployment_sources_saved(&Sources::Files(vec![root.path().join("other.php")]))
                .is_ok()
        );
        ui.documents
            .first_mut()
            .ok_or("missing document")?
            .buffer
            .mark_saved();
        assert!(
            ui.deployment_sources_saved(&Sources::Path(root.path().into()))
                .is_ok()
        );
        Ok(())
    }
}
