//! Restricted Mode controls and process-worker authorization propagation.
use nozhik_config::project_trust::ProjectTrust;
use nozhik_core::ExecutionGuard;
use std::path::PathBuf;

#[derive(Default)]
pub struct TrustUi {
    root: Option<PathBuf>,
    pub session: Option<ProjectTrust>,
    observed: bool,
    confirm: Option<String>,
    error: Option<String>,
}
impl crate::NozhikUi {
    pub(crate) fn execution_allowed(&self) -> bool {
        self.trust.session.as_ref().map_or_else(
            || self.project.is_none() && self.documents.is_empty(),
            ProjectTrust::allowed,
        )
    }
    pub(crate) fn execution_guard(&self) -> ExecutionGuard {
        self.trust.session.as_ref().map_or_else(
            || {
                if self.execution_allowed() {
                    ExecutionGuard::unrestricted()
                } else {
                    ExecutionGuard::restricted()
                }
            },
            ProjectTrust::guard,
        )
    }
    pub(crate) fn trust_tick(&mut self, ctx: &egui::Context) {
        let root = self
            .project
            .as_ref()
            .map(|p| p.root_path.clone())
            .or_else(|| {
                self.documents
                    .first()
                    .and_then(|d| d.path.parent().map(std::path::Path::to_path_buf))
            });
        let root_changed = root != self.trust.root;
        if root_changed {
            self.trust = TrustUi {
                root: root.clone(),
                ..TrustUi::default()
            };
            if let Some(root) = root {
                let wake = ctx.clone();
                match ProjectTrust::start(root, move || wake.request_repaint()) {
                    Ok(session) => self.trust.session = Some(session),
                    Err(error) => self.trust.error = Some(error.to_string()),
                }
            }
        }
        let allowed = self.execution_allowed();
        let guard = self.execution_guard();
        self.vcs.execution_guard = Some(guard.clone());
        self.linters.execution_guard = Some(guard.clone());
        if let Some(panel) = &mut self.terminal {
            panel.execution_guard = Some(guard.clone());
        }
        if let Some(panel) = &mut self.llm {
            panel.set_execution_guard(guard.clone());
        }
        if root_changed || self.trust.observed != allowed {
            self.trust.observed = allowed;
            // Drop sends cancellation; never wait for server shutdown on the UI thread.
            self.language = crate::language::LanguageUi::default();
            if !allowed {
                self.terminal = None;
                self.llm = None;
                self.linters.shutdown();
                self.docker.shutdown();
                self.docker = crate::docker_panel::DockerUi::default();
                self.editor.ai.cancel();
            }
            if let Some(manager) = &self.plugins
                && let Err(error) = manager.scan_guarded(
                    self.trust.root.clone(),
                    if self.trust.root.is_some() {
                        guard
                    } else {
                        ExecutionGuard::restricted()
                    },
                )
            {
                self.trust.error = Some(error.to_string());
            }
        }
    }
    pub(crate) fn trust_banner(&mut self, ctx: &egui::Context) {
        if self.trust.root.is_none() || self.execution_allowed() {
            return;
        }
        egui::TopBottomPanel::top("project_trust").show(ctx, |ui| {
            self.trust_controls(ui);
        });
    }

    pub(crate) fn trust_controls(&mut self, ui: &mut egui::Ui) {
        if self.trust.root.is_none() {
            return;
        }
        let allowed = self.execution_allowed();
        let status = self
            .trust
            .session
            .as_ref()
            .map(ProjectTrust::status)
            .unwrap_or_default();
        ui.horizontal_wrapped(|ui| {
            ui.strong(if allowed {
                "Trusted Project"
            } else {
                "Restricted Mode"
            });
            ui.label(if allowed {
                "External project tools are enabled."
            } else {
                "View, edit and search are available. External code is disabled."
            });
            if allowed {
                if ui.button("Revoke Trust").clicked()
                    && let Some(session) = &self.trust.session
                {
                    session.revoke();
                }
            } else if ui
                .add_enabled(
                    status.ready && !self.settings_dialog.saving(),
                    egui::Button::new("Review and Trust…"),
                )
                .clicked()
            {
                self.trust.confirm.clone_from(&status.fingerprint);
            }
        });
        if let Some(error) = &self.trust.error {
            ui.label(error);
        }
        if !status.detail.is_empty() && !allowed {
            ui.weak(&status.detail);
        }
    }

    pub(crate) fn trust_confirmation(&mut self, ctx: &egui::Context) {
        let Some(root) = self.trust.root.clone() else {
            return;
        };
        if let Some(fingerprint) = self.trust.confirm.clone() {
            let modal = crate::dialog::Dialog::new("project_trust_confirmation", [640.0, 320.0]).show(ctx, |ui| {
                ui.heading("Trust this project?");
                ui.label(root.display().to_string());
                ui.label("Plugins, language servers, terminals, Docker and project commands can execute code with your account permissions. Trust only projects and tools you have reviewed.");
                ui.label("Trust is local to this canonical folder and these execution inputs. Configuration, manifest or executable changes require a new decision. This is not an OS sandbox.");
                ui.horizontal(|ui| {
                    if ui.add_enabled(!self.settings_dialog.saving(), egui::Button::new("Trust Project")).clicked() {
                        if let Some(session) = &self.trust.session { session.approve(fingerprint.clone()); }
                        self.trust.confirm = None;
                    }
                    if ui.button("Keep Restricted").clicked() { self.trust.confirm = None; }
                });
            });
            if modal.should_close() {
                self.trust.confirm = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_plugin_host::PluginStatus;
    use std::{
        path::Path,
        time::{Duration, Instant},
    };
    type TestResult = Result<(), Box<dyn std::error::Error>>;
    fn wait(mut condition: impl FnMut() -> bool) -> TestResult {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !condition() {
            if Instant::now() > deadline {
                return Err("Restricted mode integration timed out".into());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(())
    }
    fn frame(
        shell: &mut crate::NozhikUi,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1200.0, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                shell.settings_tick(ctx);
                shell.trust_banner(ctx);
                shell.settings_window(ctx);
                shell.trust_confirmation(ctx);
            },
        )
    }

    fn label_position(output: &egui::FullOutput, label: &str) -> Option<egui::Pos2> {
        output
            .shapes
            .iter()
            .rev()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    Some(text.pos + text.galley.size() * 0.5)
                }
                _ => None,
            })
    }

    fn click(shell: &mut crate::NozhikUi, ctx: &egui::Context, label: &str) -> TestResult {
        frame(shell, ctx, vec![]);
        let output = frame(shell, ctx, vec![]);
        let pos = label_position(&output, label).ok_or_else(|| format!("Missing {label}"))?;
        for pressed in [true, false] {
            frame(
                shell,
                ctx,
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        Ok(())
    }

    #[test]
    fn settings_manage_both_trust_states_and_trusted_banner_takes_no_space() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("project");
        std::fs::create_dir(&root)?;
        let bus = nozhik_event_bus::EventBus::new(32)?;
        let settings = nozhik_config::ConfigManager::from_path(
            root.join(".nozhik/settings.toml"),
            bus.clone(),
        )?;
        let mut shell = crate::NozhikUi::new(bus, settings, nozhik_config::Config::default());
        shell.trust.root = Some(root.clone());
        shell.trust.session = Some(ProjectTrust::with_storage(
            root,
            dir.path().join("host-trust"),
            vec![],
            || {},
        )?);
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx);
        wait(|| {
            frame(&mut shell, &ctx, vec![]);
            !shell.settings_dialog.saving()
                && shell
                    .trust
                    .session
                    .as_ref()
                    .is_some_and(|s| s.status().ready)
        })?;
        let output = frame(&mut shell, &ctx, vec![]);
        assert!(label_position(&output, "Restricted Mode").is_some());
        assert!(label_position(&output, "Revoke Trust").is_none());
        shell.settings_open = true;
        // Hide only the banner in this frame to locate the Settings copy unambiguously.
        let mut output = None;
        for _ in 0..3 {
            output = Some(ctx.run(egui::RawInput::default(), |ctx| shell.settings_window(ctx)));
        }
        assert!(label_position(&output.ok_or("Settings frame")?, "Review and Trust…").is_some());
        wait(|| {
            frame(&mut shell, &ctx, vec![]);
            !shell.settings_dialog.saving()
        })?;
        click(&mut shell, &ctx, "Review and Trust…")?;
        assert!(shell.trust.confirm.is_some());
        click(&mut shell, &ctx, "Trust Project")?;
        wait(|| shell.execution_allowed())?;
        let output = frame(&mut shell, &ctx, vec![]);
        assert!(label_position(&output, "Revoke Trust").is_some());
        shell.settings_open = false;
        let output = frame(&mut shell, &ctx, vec![]);
        assert!(label_position(&output, "Trusted Project").is_none());
        assert!(label_position(&output, "Revoke Trust").is_none());
        assert_eq!(ctx.available_rect(), ctx.screen_rect());
        shell.settings_open = true;
        click(&mut shell, &ctx, "Revoke Trust")?;
        assert!(!shell.execution_allowed());
        let output = frame(&mut shell, &ctx, vec![]);
        assert!(label_position(&output, "Restricted Mode").is_some());
        assert!(label_position(&output, "Review and Trust…").is_some());
        Ok(())
    }

    #[test]
    #[allow(
        clippy::too_many_lines,
        reason = "Exercises project.open, plugin scan, read/edit, consent, revoke and changed executable in one isolated lifecycle"
    )]
    fn untrusted_project_open_never_executes_until_explicit_current_trust() -> TestResult {
        let dir = tempfile::tempdir()?;
        let root = dir.path().join("project");
        let plugin = root.join(".nozhik/plugins/fixture");
        std::fs::create_dir_all(&plugin)?;
        let root = root.canonicalize()?;
        let plugin = root.join(".nozhik/plugins/fixture");
        let marker = dir.path().join("started");
        let filename = format!("fixture{}", std::env::consts::EXE_SUFFIX);
        let binary = plugin.join(&filename);
        let source = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../plugin_host/tests/fixtures/process_plugin.rs");
        let compiled =
            std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
                .arg(source)
                .arg("-o")
                .arg(&binary)
                .output()?;
        assert!(compiled.status.success());
        let manifest = serde_json::json!({"id":"org.nozhik.fixture","name":"Fixture","version":"0.1.0",
            "min_ide_version":"0.1.0","description":"Test executable only","author":"Tests",
            "activation_events":["*"],"executable":filename,"args":["ok",marker],"contributes":{}});
        std::fs::write(plugin.join("plugin.json"), serde_json::to_vec(&manifest)?)?;
        let bus = nozhik_event_bus::EventBus::new(64)?;
        let settings = nozhik_config::ConfigManager::from_path(
            root.join(".nozhik/settings.toml"),
            bus.clone(),
        )?;
        let mut shell = crate::NozhikUi::new(bus, settings, nozhik_config::Config::default());
        let ctx = egui::Context::default();
        shell.open_project(root.clone());
        wait(|| {
            shell.receive_files();
            shell.project.is_some()
        })?;
        shell.trust.root = Some(root.clone());
        shell.trust.session = Some(ProjectTrust::with_storage(
            root.clone(),
            dir.path().join("host-trust"),
            vec![],
            || {},
        )?);
        shell.trust_tick(&ctx);
        shell.loaded_file(root.join("test.php"), "<?php echo 1;", false);
        shell.documents[0].buffer.insert(6, "// edited\n")?;
        assert!(shell.documents[0].buffer.is_dirty());
        shell.language_tick(&ctx);
        shell.docker_tick(&ctx);
        wait(|| {
            shell
                .plugins
                .as_ref()
                .is_some_and(|manager| !manager.snapshot().plugins.is_empty())
        })?;
        assert!(!shell.execution_allowed());
        assert!(!marker.exists());
        wait(|| {
            shell
                .trust
                .session
                .as_ref()
                .is_some_and(|s| s.status().ready)
        })?;
        let session = shell.trust.session.as_ref().ok_or("Trust session")?;
        session.approve(session.status().fingerprint.ok_or("Candidate")?);
        wait(|| shell.execution_allowed())?;
        shell.trust_tick(&ctx);
        wait(|| {
            shell.plugins.as_ref().is_some_and(|manager| {
                manager
                    .snapshot()
                    .plugins
                    .iter()
                    .any(|p| p.status == PluginStatus::Active)
            })
        })?;
        assert!(
            marker.exists(),
            "Explicit consent permits the reviewed fixture"
        );
        shell
            .trust
            .session
            .as_ref()
            .ok_or("Trust session")?
            .revoke();
        assert!(!shell.execution_allowed());
        shell.trust_tick(&ctx);
        wait(|| {
            shell.plugins.as_ref().is_some_and(|manager| {
                manager
                    .snapshot()
                    .plugins
                    .iter()
                    .all(|p| p.status != PluginStatus::Active)
            })
        })?;
        std::fs::remove_file(&marker)?;
        // Project configuration cannot confer trust on itself.
        std::fs::write(root.join(".nozhik/trust.json"), r#"{"trusted":true}"#)?;
        shell
            .plugins
            .as_ref()
            .ok_or("Plugin host")?
            .scan_guarded(Some(root), shell.execution_guard())?;
        std::thread::sleep(Duration::from_millis(200));
        assert!(!marker.exists());
        assert!(shell.documents[0].buffer.is_dirty());
        Ok(())
    }
}
