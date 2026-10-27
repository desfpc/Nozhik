//! Application-scoped local service ownership and bounded model cleanup.
use crate::{DEFAULT_ENDPOINT, Ollama};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{
    process::{Child, Command},
    runtime::Handle,
    sync::{oneshot, watch},
    task::JoinHandle,
};

/// Owns the background lifecycle task. Call shutdown before stopping the runtime.
pub struct Lifecycle {
    settings: watch::Sender<Option<String>>,
    stop: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<()>>,
}
impl Lifecycle {
    /// Starts/warms Ollama only when AI is enabled. Never installs Ollama or downloads models.
    #[must_use]
    pub fn start(runtime: &Handle, model: Option<String>) -> Self {
        Self::spawn(
            runtime,
            model,
            DEFAULT_ENDPOINT.into(),
            executable_candidates(),
        )
    }
    fn spawn(
        runtime: &Handle,
        model: Option<String>,
        endpoint: String,
        executables: Vec<PathBuf>,
    ) -> Self {
        let (settings, mut changes) = watch::channel(model);
        let (stop, mut stopping) = oneshot::channel();
        let task = runtime.spawn(async move {
            let Ok(client) = Ollama::new(&endpoint) else {
                return;
            };
            let mut session = Session {
                client,
                endpoint,
                executables,
                child: None,
                model: None,
            };
            loop {
                let model = changes.borrow_and_update().clone();
                tokio::select! {
                    biased;
                    _ = &mut stopping => break,
                    notice = changes.changed() => { if notice.is_err() { break; } continue; }
                    () = session.apply(model) => {}
                }
                tokio::select! {
                    _ = &mut stopping => break,
                    notice = changes.changed() => if notice.is_err() { break; }
                }
            }
            session.release().await;
        });
        Self {
            settings,
            stop: Some(stop),
            task: Some(task),
        }
    }
    /// Settings updates are coalesced; None disables AI and releases owned resources.
    #[must_use]
    pub fn controller(&self) -> watch::Sender<Option<String>> {
        self.settings.clone()
    }
    /// Cancels startup/preload, unloads the last selected model, then kills/reaps only our server.
    pub async fn shutdown(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}
impl Drop for Lifecycle {
    fn drop(&mut self) {
        // Sender drop requests asynchronous cleanup while the app runtime remains alive.
        self.stop.take();
    }
}
struct Session {
    client: Ollama,
    endpoint: String,
    executables: Vec<PathBuf>,
    child: Option<Child>,
    model: Option<String>,
}
impl Session {
    async fn apply(&mut self, model: Option<String>) {
        if self.model != model {
            self.release_model().await;
        }
        let Some(model) = model else {
            self.release().await;
            return;
        };
        // Track intent before any I/O: the user may start Ollama or download this model
        // later, and shutdown must unload subsequent editor generations as well.
        self.model = Some(model.clone());
        if self.client.models().await.is_err() && !self.start_service().await {
            return;
        }
        if self.client.models().await.is_ok_and(|models| {
            models
                .iter()
                .any(|name| canonical(name) == canonical(&model))
        }) {
            // Missing model/service is a normal offline state; generation and Settings can retry.
            let _ = self.client.residency(&model, true).await;
        }
    }
    async fn start_service(&mut self) -> bool {
        // An owned but unresponsive process must be reaped before a retry.
        if let Some(mut child) = self.child.take() {
            let _ = terminate_tree(&mut child).await;
        }
        for path in &self.executables {
            let mut command = Command::new(path);
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.as_std_mut().process_group(0);
            }
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                command.as_std_mut().creation_flags(0x0800_0000);
            }
            let result = command
                .arg("serve")
                .env("OLLAMA_HOST", &self.endpoint)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .spawn();
            if let Ok(child) = result {
                self.child = Some(child);
                break;
            }
        }
        let Some(child) = &mut self.child else {
            return false;
        };
        for _ in 0..40 {
            if !matches!(child.try_wait(), Ok(None)) {
                return false;
            }
            if self.client.models().await.is_ok() && matches!(child.try_wait(), Ok(None)) {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        false
    }
    async fn release_model(&mut self) {
        if let Some(model) = self.model.as_ref() {
            // Keep ownership until completion: cancelling cleanup must not forget this model.
            let _ = self.client.residency(model, false).await;
        }
        self.model = None;
    }
    async fn release(&mut self) {
        self.release_model().await;
        if let Some(mut child) = self.child.take() {
            // kill awaits/reaps the direct child. External services are never signalled.
            let _ = terminate_tree(&mut child).await;
        }
    }
}
async fn terminate_tree(child: &mut Child) -> std::io::Result<()> {
    if matches!(child.try_wait(), Ok(None))
        && let Some(id) = child.id()
    {
        #[cfg(unix)]
        if let Ok(id) = i32::try_from(id) {
            // The child was started in its own group, containing only our Ollama runners.
            let _ = nix::sys::signal::killpg(
                nix::unistd::Pid::from_raw(id),
                nix::sys::signal::Signal::SIGKILL,
            );
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let mut command = Command::new("taskkill");
            command.as_std_mut().creation_flags(0x0800_0000);
            let _ = tokio::time::timeout(
                Duration::from_secs(3),
                command
                    .args(["/PID", &id.to_string(), "/T", "/F"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .kill_on_drop(true)
                    .status(),
            )
            .await;
        }
    }
    child.kill().await
}
fn canonical(model: &str) -> String {
    if model.contains(':') {
        model.into()
    } else {
        format!("{model}:latest")
    }
}
fn executable_candidates() -> Vec<PathBuf> {
    let paths = vec![PathBuf::from("ollama")];
    #[cfg(target_os = "macos")]
    let paths = {
        let mut paths = paths;
        paths.extend(
            [
                "/opt/homebrew/bin/ollama",
                "/usr/local/bin/ollama",
                "/Applications/Ollama.app/Contents/Resources/ollama",
            ]
            .map(PathBuf::from),
        );
        paths
    };
    #[cfg(target_os = "windows")]
    let paths = {
        let mut paths = paths;
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            paths.push(PathBuf::from(local).join("Programs/Ollama/ollama.exe"));
        }
        paths
    };
    paths
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn server() -> Result<
        (
            String,
            JoinHandle<Vec<serde_json::Value>>,
            oneshot::Sender<()>,
        ),
        Box<dyn std::error::Error>,
    > {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let (stop, mut stopping) = oneshot::channel();
        let task = tokio::spawn(async move {
            let mut records = Vec::new();
            loop {
                let connection = tokio::select! {
                    _ = &mut stopping => break,
                    connection = listener.accept() => connection,
                };
                let Ok((mut socket, _)) = connection else {
                    break;
                };
                let mut bytes = Vec::new();
                let body = loop {
                    let mut chunk = [0; 4096];
                    let Ok(count) = socket.read(&mut chunk).await else {
                        break None;
                    };
                    if count == 0 {
                        break None;
                    }
                    bytes.extend_from_slice(&chunk[..count]);
                    let text = String::from_utf8_lossy(&bytes);
                    if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                        if headers.starts_with("GET") {
                            break Some(String::new());
                        }
                        let length = headers.lines().find_map(|line| {
                            line.to_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|s| s.parse::<usize>().ok())
                        });
                        if length.is_some_and(|n| body.len() >= n) {
                            break Some(body.to_owned());
                        }
                    }
                };
                let Some(body) = body else {
                    continue;
                };
                let response = if body.is_empty() {
                    r#"{"models":[{"name":"one:latest"},{"name":"two:latest"}]}"#
                } else {
                    if let Ok(value) = serde_json::from_str(&body) {
                        records.push(value);
                    }
                    r#"{"done":true}"#
                };
                if socket.write_all(format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{response}", response.len()).as_bytes()).await.is_err() { break; }
            }
            records
        });
        Ok((endpoint, task, stop))
    }

    #[tokio::test]
    async fn external_service_survives_model_switch_and_shutdown()
    -> Result<(), Box<dyn std::error::Error>> {
        let (endpoint, server, stop) = server().await?;
        let mut session = Session {
            client: Ollama::new(&endpoint)?,
            endpoint,
            executables: Vec::new(),
            child: None,
            model: None,
        };
        session.apply(Some("one".into())).await;
        session.apply(Some("two".into())).await;
        session.release().await;
        assert!(
            session.client.models().await.is_ok(),
            "external service must remain alive"
        );
        let _ = stop.send(());
        let records = server.await?;
        let sequence: Vec<_> = records
            .iter()
            .map(|r| (r["model"].as_str(), r["keep_alive"].as_str()))
            .collect();
        assert_eq!(
            sequence,
            [
                (Some("one"), Some("5m")),
                (Some("one"), Some("0")),
                (Some("two"), Some("5m")),
                (Some("two"), Some("0"))
            ]
        );
        assert!(records.iter().all(|r| r.get("prompt").is_none()));
        Ok(())
    }

    #[tokio::test]
    async fn owned_process_stops_when_disabled_and_on_close()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let executable = directory.path().join("ollama");
        let script = "#!/bin/sh\nexec sleep 120\n";
        std::fs::write(&executable, script)?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
        let (endpoint, server, stop) = server().await?;
        let mut session = Session {
            client: Ollama::new(&endpoint)?,
            endpoint,
            executables: vec![executable],
            child: None,
            model: None,
        };
        // Exercise real owned subprocess creation/reaping without touching a real Ollama install.
        assert!(session.start_service().await);
        assert!(
            session
                .child
                .as_mut()
                .is_some_and(|child| matches!(child.try_wait(), Ok(None)))
        );
        let first_pid = session
            .child
            .as_ref()
            .and_then(Child::id)
            .and_then(|id| i32::try_from(id).ok());
        session.apply(None).await;
        assert!(session.child.is_none());
        if let Some(pid) = first_pid {
            assert_eq!(
                nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
                Err(nix::errno::Errno::ESRCH)
            );
        }
        assert!(session.start_service().await);
        session.release().await;
        assert!(session.child.is_none());
        let _ = stop.send(());
        server.await?;
        Ok(())
    }

    #[tokio::test]
    async fn disabled_lifecycle_does_not_launch_and_shutdown_interrupts_startup()
    -> Result<(), Box<dyn std::error::Error>> {
        let directory = tempfile::tempdir()?;
        let executable = directory.path().join("ollama");
        let marker = directory.path().join("started");
        std::fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf started > '{}'\nexec sleep 120\n",
                marker.display()
            ),
        )?;
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700))?;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        drop(listener);
        let lifecycle = Lifecycle::spawn(&Handle::current(), None, endpoint, vec![executable]);
        tokio::time::sleep(Duration::from_millis(30)).await;
        assert!(!marker.exists());
        lifecycle.controller().send_replace(Some("one".into()));
        tokio::time::timeout(Duration::from_secs(3), async {
            while !marker.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await?;
        tokio::time::timeout(Duration::from_secs(2), lifecycle.shutdown()).await?;
        Ok(())
    }
}
