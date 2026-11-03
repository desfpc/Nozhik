//! Explicit, isolated Docker smoke test; no ports, mounts, pulls or user containers.
use nozhik_docker::{Action, Command, DockerWorker, Event, Response};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
type TestResult = Result<(), Box<dyn std::error::Error>>;
struct Cleanup {
    file: PathBuf,
}
impl Drop for Cleanup {
    fn drop(&mut self) {
        let _cleanup = std::process::Command::new("docker")
            .args(["compose", "-f"])
            .arg(&self.file)
            .args(["down", "--remove-orphans"])
            .output();
    }
}
fn request(
    worker: &DockerWorker,
    file: &std::path::Path,
    token: u64,
    action: Option<Action>,
) -> Result<Response, Box<dyn std::error::Error>> {
    let files = vec![file.into()];
    let command = match action {
        Some(action) => Command::Act {
            token,
            files,
            action,
        },
        None => Command::Refresh { token, files },
    };
    assert!(worker.send(command));
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        for event in worker.drain() {
            if let Event::Completed(actual, result) = event {
                assert_eq!(actual, token);
                return Ok(result?);
            }
        }
        if Instant::now() >= deadline {
            return Err("Docker integration timeout".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
#[test]
#[ignore = "Requires a running Docker engine, Compose and a locally cached shell image; set NOZHIK_DOCKER_TEST_IMAGE"]
fn isolated_compose_lifecycle_logs_and_console() -> TestResult {
    let dir = tempfile::tempdir()?;
    let root = dir.path().canonicalize()?;
    let file = root.join("compose.yaml");
    let name = format!(
        "nozhik-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_millis()
    );
    let image =
        std::env::var("NOZHIK_DOCKER_TEST_IMAGE").unwrap_or_else(|_| "busybox:latest".into());
    std::fs::write(
        &file,
        format!(
            "name: {name}\nservices:\n  smoke:\n    image: {image}\n    pull_policy: never\n    network_mode: none\n    command: ['/bin/sh', '-c', 'echo fixture-ready; while :; do sleep 1; done']\n"
        ),
    )?;
    let _cleanup = Cleanup { file: file.clone() };
    let worker = DockerWorker::start(root, || {});
    let Response::Snapshot(initial) = request(&worker, &file, 1, None)? else {
        return Err("snapshot".into());
    };
    assert_eq!(initial.project, name);
    assert_eq!(initial.containers.len(), 0);
    let Response::Snapshot(started) = request(&worker, &file, 2, Some(Action::Up(None)))? else {
        return Err("up".into());
    };
    let container = started.containers.first().ok_or("container")?;
    assert_eq!(container.state, "running");
    let id = container.id.clone();
    let Response::Logs(logs) = request(&worker, &file, 3, Some(Action::Logs(id.clone())))? else {
        return Err("logs".into());
    };
    assert!(logs.contains("fixture-ready"));
    let Response::Console { program, args, .. } =
        request(&worker, &file, 4, Some(Action::Console(id.clone())))?
    else {
        return Err("console".into());
    };
    // Exercise the exact validated exec target without a PTY, using a one-shot shell.
    let mut args: Vec<_> = args.into_iter().filter(|arg| arg != "-it").collect();
    args.extend(["-c".into(), "printf 'console-ready'".into()]);
    let output = std::process::Command::new(program).args(args).output()?;
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"console-ready");
    let Response::Snapshot(stopped) = request(&worker, &file, 5, Some(Action::Stop(id.clone())))?
    else {
        return Err("stop".into());
    };
    assert_eq!(stopped.containers[0].state, "exited");
    let Response::Snapshot(restarted) = request(&worker, &file, 6, Some(Action::Start(id)))? else {
        return Err("start".into());
    };
    assert_eq!(restarted.containers[0].state, "running");
    let Response::Snapshot(stopped) = request(&worker, &file, 7, Some(Action::StopAll))? else {
        return Err("stop all".into());
    };
    assert_eq!(stopped.containers[0].state, "exited");
    Ok(())
}
