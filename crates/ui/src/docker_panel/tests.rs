use super::*;
use crate::{Document, LeftTool, NozhikUi};
use egui::{Event as Input, Pos2, Rect, vec2};
type TestResult = Result<(), Box<dyn std::error::Error>>;
fn frame(shell: &mut NozhikUi, ctx: &egui::Context, events: Vec<Input>) -> egui::FullOutput {
    ctx.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1100.0, 760.0))),
            events,
            ..Default::default()
        },
        |ctx| {
            shell.docker_tick(ctx);
            if let Some(terminal) = &mut shell.terminal {
                terminal.tick();
            }
            shell.panels(ctx);
            shell.editor_area(ctx);
            shell.docker_window(ctx);
        },
    )
}
fn click(shell: &mut NozhikUi, ctx: &egui::Context, pos: Pos2) {
    for pressed in [true, false] {
        frame(
            shell,
            ctx,
            vec![
                Input::PointerMoved(pos),
                Input::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
    }
}
fn text(output: &egui::FullOutput, label: &str) -> Option<Pos2> {
    output.shapes.iter().find_map(|s| match &s.shape {
        egui::Shape::Text(t) if t.galley.text() == label => Some(t.pos + t.galley.size() * 0.5),
        _ => None,
    })
}
fn wait(
    shell: &mut NozhikUi,
    ctx: &egui::Context,
    ready: impl Fn(&NozhikUi) -> bool,
) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        frame(shell, ctx, vec![]);
        if ready(shell) {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!("Docker UI timeout: {:?}", shell.docker.error).into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}
fn fixture() -> Result<(tempfile::TempDir, NozhikUi, egui::Context), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let program = root.join(format!("mock-docker{}", std::env::consts::EXE_SUFFIX));
    let output = std::process::Command::new("rustc")
        .arg("--edition=2024")
        .arg(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../docker/tests/fixtures/mock_docker.rs"),
        )
        .arg("-o")
        .arg(&program)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source = "services:\n  web:\n    image: example\n";
    let path = root.join("docker-compose.yml");
    std::fs::write(&path, source)?;
    std::fs::write(
        root.join("mock-config.json"),
        r#"{"name":"example","services":{"web":{"image":"example"}}}"#,
    )?;
    std::fs::write(root.join("mock-ps.json"), r#"{"ID":"aaaaaaaaaaaa"}"#)?;
    std::fs::write(
        root.join("mock-inspect.json"),
        serde_json::to_string(
            &serde_json::json!([{"Id":"aaaaaaaaaaaa","Name":"/example-web-1","Config":{"Image":"example","Labels":{"com.docker.compose.project":"example","com.docker.compose.project.working_dir":root,"com.docker.compose.service":"web"}},"State":{"Status":"running","ExitCode":0},"Mounts":[]}]),
        )?,
    )?;
    let bus = nozhik_event_bus::EventBus::new(32)?;
    let settings =
        nozhik_config::ConfigManager::from_path(root.join("settings.toml"), bus.clone())?;
    let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
    shell.project = Some(nozhik_fs_model::tree::ProjectTree::new(&root)?);
    shell.documents.push(Document {
        path,
        binary: false,
        name: "docker-compose.yml".into(),
        open: true,
        buffer: nozhik_buffer::TextBuffer::load_from_str(nozhik_core::BufferId(500), source),
    });
    shell.active = Some(0);
    shell.left = Some(LeftTool::Docker);
    shell.docker.root = Some(root.clone());
    shell.docker.worker = Some(DockerWorker::with_program(root, Some(program), || {}));
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    Ok((dir, shell, ctx))
}
#[test]
fn docker_gutter_actions_dirty_gate_controls_logs_and_terminal_tab() -> TestResult {
    let (dir, mut shell, ctx) = fixture()?;
    wait(&mut shell, &ctx, |s| {
        s.docker.snapshot.is_some() && s.editor.docker_rects.len() == 2
    })?;
    let run = shell
        .editor
        .docker_rects
        .iter()
        .find(|(m, _)| m.service.as_deref() == Some("web"))
        .ok_or("service marker")?
        .1
        .center();
    click(&mut shell, &ctx, run);
    wait(&mut shell, &ctx, |s| {
        s.docker.busy.is_none() && dir.path().join("mock-up").exists()
    })?;
    assert!(std::fs::read_to_string(dir.path().join("mock-up"))?.ends_with("--\tweb"));
    std::fs::remove_file(dir.path().join("mock-up"))?;
    let run = shell
        .editor
        .docker_rects
        .iter()
        .find(|(m, _)| m.service.is_none())
        .ok_or("group marker")?
        .1
        .center();
    click(&mut shell, &ctx, run);
    wait(&mut shell, &ctx, |s| {
        s.docker.busy.is_none() && dir.path().join("mock-up").exists()
    })?;
    assert!(std::fs::read_to_string(dir.path().join("mock-up"))?.ends_with("--"));
    std::fs::remove_file(dir.path().join("mock-up"))?;
    shell.documents[0].buffer.insert(0, "# changed\n")?;
    wait(&mut shell, &ctx, |s| {
        s.editor
            .docker_rects
            .first()
            .is_some_and(|(m, _)| m.line == 1)
    })?;
    let run = shell.editor.docker_rects[0].1.center();
    click(&mut shell, &ctx, run);
    frame(&mut shell, &ctx, vec![]);
    assert!(shell.status.contains("Save the Compose file"));
    assert!(!dir.path().join("mock-up").exists());
    shell.documents[0].buffer.undo()?;
    let output = frame(&mut shell, &ctx, vec![]);
    let selected = text(&output, "example-web-1").ok_or("container row")?;
    click(&mut shell, &ctx, selected);
    let output = frame(&mut shell, &ctx, vec![]);
    let logs = text(&output, "Logs").ok_or("logs tab")?;
    click(&mut shell, &ctx, logs);
    wait(&mut shell, &ctx, |s| s.docker.logs.contains("stderr log"))?;
    let output = frame(&mut shell, &ctx, vec![]);
    let console = text(&output, "Open console").ok_or("console button")?;
    click(&mut shell, &ctx, console);
    wait(&mut shell, &ctx, |s| {
        s.bottom == Some(crate::terminal_panel::TERMINAL_LABEL)
    })?;
    verify_console(&mut shell, &ctx)?;
    if let Some(panel) = &mut shell.terminal {
        panel.shutdown();
    }
    shell.docker.shutdown();
    Ok(())
}

#[test]
fn docker_rail_requires_compose_file_inventory() -> TestResult {
    let dir = tempfile::tempdir()?;
    let bus = nozhik_event_bus::EventBus::new(8)?;
    let settings =
        nozhik_config::ConfigManager::from_path(dir.path().join("settings.toml"), bus.clone())?;
    let mut shell = NozhikUi::new(bus, settings, nozhik_config::Config::default());
    let ctx = egui::Context::default();
    crate::theme::apply(&ctx);
    shell.left = Some(LeftTool::Project);
    frame(&mut shell, &ctx, vec![]);
    let rail = egui::containers::panel::PanelState::load(&ctx, egui::Id::new("left_strip"))
        .ok_or("rail")?
        .rect;
    let docker = rail.left_top() + vec2(18.0, 146.0);
    click(&mut shell, &ctx, docker);
    assert!(shell.left == Some(LeftTool::Project));
    shell.docker.files.push(dir.path().join("compose.yaml"));
    frame(&mut shell, &ctx, vec![]);
    click(&mut shell, &ctx, docker);
    assert!(shell.left == Some(LeftTool::Docker));
    Ok(())
}

fn verify_console(shell: &mut NozhikUi, ctx: &egui::Context) -> TestResult {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let output = frame(shell, ctx, vec![]);
        let painted: String = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text()),
                _ => None,
            })
            .collect();
        if text(&output, "[Docker: example-web-1]").is_some()
            && painted.contains("CONTAINER SHELL READY")
        {
            break;
        }
        if Instant::now() > deadline {
            return Err("Docker PTY not painted".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    frame(
        shell,
        ctx,
        vec![
            Input::Text("ping".into()),
            Input::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    loop {
        let output = frame(shell, ctx, vec![]);
        let painted: String = output
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some(t.galley.text()),
                _ => None,
            })
            .collect();
        if painted.contains("docker-shell:ping") {
            break;
        }
        if Instant::now() > deadline {
            return Err("Docker console input not delivered".into());
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Ok(())
}

#[test]
fn refresh_does_not_shift_container_list() {
    for width in [180.0, 280.0, 420.0] {
        let ctx = egui::Context::default();
        let mut state = DockerUi::default();
        let mut idle_position = None;
        for (busy, user_operation) in [(false, false), (true, false), (true, true), (false, false)]
        {
            state.busy = busy.then_some(1);
            state.user_operation = user_operation;
            // Allow egui's layout sizing to settle before comparing frames.
            for pass in 0..2 {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 600.0))),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            panel_header(ui, &mut state);
                            ui.label("Container list");
                        });
                    },
                );
                assert!(text(&output, "Docker operation…").is_none());
                assert!(text(&output, "Cancel").is_some());
                let position = text(&output, "Container list");
                assert!(position.is_some());
                if pass == 1 {
                    if let Some(idle) = idle_position {
                        assert_eq!(position, Some(idle), "toolbar shifted at width {width}");
                    } else {
                        idle_position = position;
                    }
                }
            }
        }
    }
}

#[test]
fn spinner_only_appears_for_user_requests() {
    let ctx = egui::Context::default();
    let mut state = DockerUi::default();
    for (busy, user_operation) in [(false, false), (true, false), (true, true), (false, true)] {
        state.busy = busy.then_some(1);
        state.user_operation = user_operation;
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                operation_indicator(ui, &state);
            });
        });
        let spinner_paths = output
            .shapes
            .iter()
            .filter(|shape| matches!(shape.shape, egui::Shape::Path(_)))
            .count();
        assert_eq!(spinner_paths, usize::from(busy && user_operation));
    }
}
