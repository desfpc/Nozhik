use super::*;
#[test]
fn names_markers_and_configuration() -> Result<()> {
    use std::path::Path;
    for name in [
        "docker-compose.yml",
        "docker-compose.prod.yaml",
        "compose.yml",
        "compose.override.yaml",
        "COMPOSE.YAML",
    ] {
        assert!(is_compose_file(Path::new(name)));
    }
    for name in [
        "docker-compose.yml.bak",
        "docker-compose-not.yml",
        "compose.txt",
        "bitbucket-pipelines.yml",
    ] {
        assert!(!is_compose_file(Path::new(name)));
    }
    let source = "# services: fake\nname: demo\nx-example: |\n  services:\n    fake:\nservices:\n  web:\n    image: nginx\n  'php-fpm':\n    image: php\n";
    assert_eq!(
        compose::run_markers(source)?,
        vec![
            RunMarker {
                line: 5,
                service: None
            },
            RunMarker {
                line: 6,
                service: Some("web".into())
            },
            RunMarker {
                line: 8,
                service: Some("php-fpm".into())
            }
        ]
    );
    assert_eq!(compose::run_markers("services: [broken")?.len(), 0);
    let files = [
        "/repo/compose.yaml",
        "/repo/compose.override.yaml",
        "/repo/compose.prod.yaml",
    ]
    .map(PathBuf::from);
    assert_eq!(compose::configuration(&files[0], &files), files[..2]);
    assert_eq!(
        compose::configuration(&files[2], &files),
        vec![files[0].clone(), files[2].clone()]
    );
    assert_eq!(
        engine::json_rows("{\"ID\":\"a\"}\n{\"ID\":\"b\"}\n")?.len(),
        2
    );
    assert_eq!(engine::json_rows("[{\"ID\":\"a\"}]")?.len(), 1);
    assert!(engine::valid_id("--malicious").is_err());
    Ok(())
}

fn fixture() -> std::result::Result<(tempfile::TempDir, PathBuf), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let executable = root.join(format!("mock-docker{}", std::env::consts::EXE_SUFFIX));
    let output = std::process::Command::new("rustc")
        .args(["--edition=2024"])
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/mock_docker.rs"))
        .arg("-o")
        .arg(&executable)
        .output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    std::fs::write(
        root.join("compose.yaml"),
        "services:\n  web:\n    image: example\n",
    )?;
    std::fs::write(
        root.join("mock-config.json"),
        r#"{"name":"example","services":{"web":{"image":"example"},"db":{"image":"postgres"}}}"#,
    )?;
    std::fs::write(root.join("mock-ps.json"), r#"{"ID":"aaaaaaaaaaaa"}"#)?;
    std::fs::write(
        root.join("mock-inspect.json"),
        serde_json::to_string(
            &serde_json::json!([{"Id":"aaaaaaaaaaaa","Name":"/example-web-1","Config":{"Image":"example","Labels":{"com.docker.compose.project":"example","com.docker.compose.project.working_dir":root,"com.docker.compose.service":"web"}},"State":{"Status":"running","ExitCode":0},"Mounts":[{"Source":root,"Destination":"/app"}]}]),
        )?,
    )?;
    Ok((dir, executable))
}
#[test]
fn cli_scope_controls_logs_and_console_arguments()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, program) = fixture()?;
    let root = dir.path().canonicalize()?;
    let engine = engine::Engine::new(
        root.clone(),
        program.clone(),
        Arc::new(AtomicBool::new(false)),
    );
    let files = vec![root.join("compose.yaml")];
    let snapshot = engine.snapshot(&files)?;
    assert_eq!(snapshot.services.len(), 2);
    assert_eq!(snapshot.containers[0].name, "example-web-1");
    let response = engine.act(&files, &Action::Console("aaaaaaaaaaaa".into()))?;
    let Response::Console {
        program: actual,
        args,
        name,
    } = response
    else {
        return Err("console response".into());
    };
    assert_eq!(actual, program);
    assert_eq!(name, "example-web-1");
    assert_eq!(
        args,
        ["exec", "-it", "--", "aaaaaaaaaaaa", "/bin/sh"].map(OsString::from)
    );
    let Response::Logs(logs) = engine.act(&files, &Action::Logs("aaaaaaaaaaaa".into()))? else {
        return Err("logs response".into());
    };
    assert!(logs.contains("stdout log") && logs.contains("stderr log"));
    assert!(logs.find("stdout log") < logs.find("stderr log"));
    assert!(logs.find("stderr log") < logs.find("stdout after"));
    engine.act(&files, &Action::Stop("aaaaaaaaaaaa".into()))?;
    assert_eq!(engine.snapshot(&files)?.containers[0].state, "exited");
    assert!(
        engine
            .act(&files, &Action::Console("aaaaaaaaaaaa".into()))
            .is_err()
    );
    engine.act(&files, &Action::Start("aaaaaaaaaaaa".into()))?;
    assert_eq!(engine.snapshot(&files)?.containers[0].state, "running");
    engine.act(&files, &Action::Up(Some("db".into())))?;
    assert!(std::fs::read_to_string(root.join("mock-up"))?.ends_with("--\tdb"));
    engine.act(&files, &Action::StopAll)?;
    assert!(
        engine
            .act(&files, &Action::Start("bbbbbbbbbbbb".into()))
            .is_err()
    );
    assert!(
        engine
            .act(&files, &Action::Up(Some("--build".into())))
            .is_err()
    );
    let foreign = tempfile::tempdir()?;
    let path = root.join("mock-inspect.json");
    let mut data: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
    data[0]["Config"]["Labels"]["com.docker.compose.project.working_dir"] =
        foreign.path().display().to_string().into();
    std::fs::write(path, serde_json::to_string(&data)?)?;
    std::fs::remove_file(root.join("mock-up"))?;
    assert!(engine.act(&files, &Action::Up(None)).is_err());
    assert!(!root.join("mock-up").exists());
    assert!(
        engine
            .snapshot(&[foreign.path().join("compose.yaml")])
            .is_err()
    );
    Ok(())
}
#[test]
fn worker_discovery_revision_errors_and_cancellation()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let (dir, program) = fixture()?;
    let root = dir.path().canonicalize()?;
    std::fs::create_dir(root.join(".git"))?;
    std::fs::write(root.join(".git/compose.yaml"), "services: {}")?;
    assert_eq!(compose::discover(&root)?, vec![root.join("compose.yaml")]);
    let worker = DockerWorker::with_program(root.clone(), Some(program), || {});
    assert!(worker.send(Command::Document {
        path: root.join("compose.yaml"),
        buffer: 7,
        revision: 4,
        source: "services:\n  web:\n    image: nginx\n".into()
    }));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    let mut found = false;
    let mut markers = false;
    while !(found && markers) {
        for event in worker.drain() {
            match event {
                Event::Files(files) => {
                    assert_eq!(files?.len(), 1);
                    found = true;
                }
                Event::Markers {
                    buffer,
                    revision,
                    markers: items,
                    ..
                } => {
                    assert_eq!((buffer, revision), (7, 4));
                    assert_eq!(items.len(), 2);
                    markers = true;
                }
                Event::Completed(..) => {}
            }
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    std::fs::write(root.join("mock-slow"), "")?;
    assert!(worker.send(Command::Act {
        token: 9,
        files: vec![root.join("compose.yaml")],
        action: Action::Up(None)
    }));
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if std::fs::read_to_string(root.join("mock-commands.log"))
            .unwrap_or_default()
            .contains("up\t--detach")
        {
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    worker.cancel();
    loop {
        if let Some(Event::Completed(token, result)) = worker.drain().next() {
            assert_eq!(token, 9);
            assert!(result.is_err());
            break;
        }
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(!root.join("mock-up").exists());
    std::thread::sleep(Duration::from_millis(1200));
    assert!(
        !root.join("mock-child-finished").exists(),
        "cancel must terminate the Compose plugin child too"
    );
    Ok(())
}
