//! Local Ollama FIM generation and explicit model management. No UI or filesystem I/O.
pub mod commit_message;
pub mod fim;
pub mod lifecycle;
pub mod ollama;

pub use commit_message::{CommitProgress, StagedFile};
pub use ollama::{Error, Ollama, PullProgress};
use std::time::Duration;
use tokio::{runtime::Handle, sync::oneshot, task::AbortHandle};
/// Default local service URL.
pub const DEFAULT_ENDPOINT: &str = "http://127.0.0.1:11434";
/// Default lightweight base model supporting Qwen FIM tokens.
pub const DEFAULT_MODEL: &str = "qwen2.5-coder:1.5b-base";

/// Completion failure category suitable for a concise editor status.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum GenerationError {
    /// The selected model must be downloaded explicitly.
    #[error("Model not installed. Open Settings → AI Assistant → Download Model")]
    ModelMissing,
    /// No reachable Ollama service.
    #[error("Ollama is starting or unavailable")]
    Unavailable,
    /// Cold load or generation exceeded its deadline.
    #[error("Model loading or generation timed out")]
    Timeout,
    /// Other server or transport failure.
    #[error("{0}")]
    Transport(String),
    /// The runtime stopped before delivering a result.
    #[error("Completion task stopped")]
    Stopped,
}

/// One owned, cancellable debounce + HTTP operation. Dropping it aborts both phases.
pub struct Generation {
    abort: AbortHandle,
    result: oneshot::Receiver<Result<Option<String>, GenerationError>>,
}
impl Generation {
    /// Schedules generation on the application's runtime; callback wakes the event loop.
    #[must_use]
    pub fn start(
        runtime: &Handle,
        client: Ollama,
        model: String,
        prompt: String,
        delay: Duration,
        wake: impl FnOnce() + Send + 'static,
    ) -> Self {
        let (sender, result) = oneshot::channel();
        let task = runtime.spawn(async move {
            tokio::time::sleep(delay).await;
            let text = client
                .generate(&model, &prompt)
                .await
                .map(|text| (!text.trim().is_empty()).then_some(text))
                .map_err(|error| match &error {
                    Error::Http(http) if http.status() == Some(reqwest::StatusCode::NOT_FOUND) => {
                        GenerationError::ModelMissing
                    }
                    Error::Http(http) if http.is_connect() => GenerationError::Unavailable,
                    Error::Http(http) if http.is_timeout() => GenerationError::Timeout,
                    _ => GenerationError::Transport(error.to_string()),
                });
            // The editor may have been closed; undeliverable completions are disposable.
            let _ = sender.send(text);
            wake();
        });
        Self {
            abort: task.abort_handle(),
            result,
        }
    }
    /// Polls once without blocking; outer None means still pending.
    pub fn poll(&mut self) -> Option<Result<Option<String>, GenerationError>> {
        match self.result.try_recv() {
            Ok(value) => Some(value),
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => Some(Err(GenerationError::Stopped)),
        }
    }
}
impl Drop for Generation {
    fn drop(&mut self) {
        self.abort.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn request_body(socket: &mut tokio::net::TcpStream) -> std::io::Result<Vec<u8>> {
        let mut request = Vec::new();
        loop {
            let mut bytes = [0; 4096];
            let count = socket.read(&mut bytes).await?;
            assert!(count > 0);
            request.extend_from_slice(&bytes[..count]);
            let text = String::from_utf8_lossy(&request);
            if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                let length = headers.lines().find_map(|line| {
                    line.to_lowercase()
                        .strip_prefix("content-length: ")
                        .and_then(|n| n.parse::<usize>().ok())
                });
                if length.is_some_and(|length| body.len() >= length) {
                    return Ok(body.as_bytes().to_vec());
                }
            }
            assert!(request.len() < 512 * 1024);
        }
    }

    fn staged(path: &str, diff: &str) -> StagedFile {
        StagedFile {
            path: path.into(),
            old_path: None,
            status: "modified".into(),
            binary: false,
            insertions: 1,
            deletions: 1,
            patch: diff.into(),
            truncated: false,
            hunks: vec!["fn kept".into()],
        }
    }

    fn prompt_of(json: &serde_json::Value) -> &str {
        json["prompt"].as_str().unwrap_or("")
    }

    fn requires(json: &serde_json::Value, field: &str) -> bool {
        json["format"]["required"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item.as_str() == Some(field)))
    }

    async fn serve_commit(
        listener: tokio::net::TcpListener,
        prompts: std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>,
        respond: impl Fn(&serde_json::Value) -> (String, bool, &'static str) + Send + 'static,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        loop {
            let accepted = tokio::time::timeout(Duration::from_secs(2), listener.accept()).await;
            let Ok(Ok((mut socket, _))) = accepted else {
                break;
            };
            let body = request_body(&mut socket).await?;
            let json: serde_json::Value = serde_json::from_slice(&body)?;
            assert_eq!(json["model"], "selected-local-model");
            assert_eq!(json["stream"], false);
            assert!(json["options"]["stop"].is_null());
            assert_eq!(json["options"]["num_ctx"], 16384);
            prompts
                .lock()
                .map_err(|_| "prompt lock")?
                .push(json.clone());
            let (response, done, reason) = respond(&json);
            let body =
                serde_json::json!({"response": response, "done": done, "done_reason": reason})
                    .to_string();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await?;
        }
        Ok(())
    }

    #[tokio::test]
    #[ignore = "Requires a running local Ollama and the installed default model"]
    async fn live_ollama_commit_message() -> Result<(), Box<dyn std::error::Error>> {
        let client = Ollama::new(DEFAULT_ENDPOINT)?;
        let message = client
            .commit_message(
                DEFAULT_MODEL,
                &[staged(
                    "math.rs",
                    "@@ -1 +1 @@ fn add\n-fn add(a: i32, b: i32) -> i32 { a - b }\n+fn add(a: i32, b: i32) -> i32 { a + b }\n",
                )],
                |_| {},
            )
            .await?;
        assert_ne!(message.summary, "");
        assert_ne!(message.description, "");
        println!("{}\n\n{}", message.summary, message.description);
        Ok(())
    }

    #[tokio::test]
    async fn commit_message_summarizes_every_file_before_the_title()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = Ollama::new(&format!("http://{}", listener.local_addr()?))?;
        let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = prompts.clone();
        let server = tokio::spawn(serve_commit(listener, prompts, |json| {
            let prompt = prompt_of(json);
            if requires(json, "files") {
                let notes = if prompt.contains("ALPHA_ONLY_IN_PATCH") {
                    serde_json::json!({"files":[{"path":"alpha.rs","note":"FILE_NOTE alpha.rs"}]})
                } else {
                    serde_json::json!({"files":[
                        {"path":"beta.rs","note":"FILE_NOTE beta.rs"},
                        {"path":"gamma.rs","note":"FILE_NOTE gamma.rs"}
                    ]})
                };
                return (notes.to_string(), true, "stop");
            }
            (
                serde_json::json!({
                    "summary": "Fix alpha, beta, and gamma",
                    "description": "- Update alpha.rs\n- Update beta.rs\n- Update gamma.rs"
                })
                .to_string(),
                true,
                "stop",
            )
        }));
        let mut labels = Vec::new();
        let message = tokio::time::timeout(
            Duration::from_secs(5),
            client.commit_message(
                "selected-local-model",
                &[
                    staged(
                        "alpha.rs",
                        &format!("+first\n{}\n", "ALPHA_ONLY_IN_PATCH".repeat(400)),
                    ),
                    staged("beta.rs", "+second BETA_ONLY_IN_PATCH\n"),
                    staged("gamma.rs", "+third GAMMA_ONLY_IN_PATCH\n"),
                ],
                |progress| labels.push(progress.label),
            ),
        )
        .await??;
        assert_eq!(message.summary, "Fix alpha, beta, and gamma");
        {
            let prompts = recorded.lock().map_err(|_| "prompt lock")?.clone();
            let maps = prompts
                .iter()
                .filter(|json| requires(json, "files"))
                .collect::<Vec<_>>();
            let finals = prompts
                .iter()
                .filter(|json| requires(json, "description"))
                .collect::<Vec<_>>();
            let map_prompt = maps.first().ok_or("missing file summary")?;
            let final_prompt = finals.first().ok_or("missing final prompt")?;
            assert_eq!(maps.len(), 2);
            assert_eq!(finals.len(), 1);
            let map_text = maps
                .iter()
                .copied()
                .map(prompt_of)
                .collect::<Vec<_>>()
                .join("\n");
            let final_text = prompt_of(final_prompt);
            assert!(map_text.contains("ALPHA_ONLY_IN_PATCH"));
            assert!(map_text.contains("BETA_ONLY_IN_PATCH"));
            assert!(map_text.contains("GAMMA_ONLY_IN_PATCH"));
            assert!(final_text.contains("FILE_NOTE alpha.rs"));
            assert!(final_text.contains("FILE_NOTE beta.rs"));
            assert!(final_text.contains("FILE_NOTE gamma.rs"));
            assert!(!final_text.contains("ALPHA_ONLY_IN_PATCH"));
            assert!(!final_text.contains("BETA_ONLY_IN_PATCH"));
            assert_eq!(final_prompt["options"]["num_predict"], 1024);
            assert_eq!(final_prompt["options"]["temperature"], 0.2);
            assert_eq!(map_prompt["options"]["temperature"], 0.1);
        }
        assert!(
            labels
                .iter()
                .any(|label| label.contains("Summarizing files (1/2)"))
        );
        assert!(
            labels
                .iter()
                .any(|label| label.contains("Writing commit message"))
        );
        server.await?.map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn commit_message_rejects_invalid_final_replies() -> Result<(), Box<dyn std::error::Error>>
    {
        for (summary, done, reason, valid) in [
            ("Add folder ignores", true, "stop", true),
            ("", true, "stop", false),
            ("line\nbreak", true, "stop", false),
            ("Incomplete", false, "stop", false),
            ("Cut off", true, "length", false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let client = Ollama::new(&format!("http://{}", listener.local_addr()?))?;
            let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let server = tokio::spawn(serve_commit(listener, prompts, move |json| {
                if requires(json, "files") {
                    return (
                        serde_json::json!({"files":[{"path":"kept.rs","note":"FILE_NOTE kept.rs"}]})
                            .to_string(),
                        true,
                        "stop",
                    );
                }
                (
                    serde_json::json!({"summary": summary, "description": "Explain changes\nwith details"})
                        .to_string(),
                    done,
                    reason,
                )
            }));
            let message = tokio::time::timeout(
                Duration::from_secs(5),
                client.commit_message(
                    "selected-local-model",
                    &[staged("kept.rs", "+staged change\n")],
                    |_| {},
                ),
            )
            .await??;
            if valid {
                assert_eq!(message.summary, summary);
                assert_eq!(message.description, "Explain changes\nwith details");
            } else {
                assert_ne!(message.summary, summary);
                assert!(message.description.contains("kept.rs"));
                assert!(
                    !message
                        .summary
                        .to_ascii_lowercase()
                        .contains("previous reply")
                );
            }
            server.await?.map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn echoed_prompt_instruction_is_replaced_by_the_file_notes()
    -> Result<(), Box<dyn std::error::Error>> {
        let echo =
            "The previous reply omitted or renamed a path. Copy each required path unchanged.";
        for note_is_echo in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let client = Ollama::new(&format!("http://{}", listener.local_addr()?))?;
            let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
            let recorded = prompts.clone();
            let echo_note = echo.to_owned();
            let server = tokio::spawn(serve_commit(listener, prompts, move |json| {
                if requires(json, "files") {
                    let note = if note_is_echo {
                        echo_note.clone()
                    } else {
                        "FILE_NOTE kept.rs".to_owned()
                    };
                    return (
                        serde_json::json!({"files":[{"path":"kept.rs","note":note}]}).to_string(),
                        true,
                        "stop",
                    );
                }
                (
                    serde_json::json!({
                        "summary": echo_note.clone(),
                        "description": echo_note.clone()
                    })
                    .to_string(),
                    true,
                    "stop",
                )
            }));
            let message = tokio::time::timeout(
                Duration::from_secs(5),
                client.commit_message(
                    "selected-local-model",
                    &[staged("kept.rs", "@@ -1 +1 @@ fn kept\n-a\n+b\n")],
                    |_| {},
                ),
            )
            .await??;
            assert!(
                !message
                    .summary
                    .to_ascii_lowercase()
                    .contains("previous reply")
            );
            assert!(
                !message
                    .description
                    .to_ascii_lowercase()
                    .contains("previous reply")
            );
            assert!(message.description.contains("kept.rs"));
            {
                let prompts = recorded.lock().map_err(|_| "prompt lock")?.clone();
                let final_text = prompts
                    .iter()
                    .filter(|json| requires(json, "description"))
                    .map(prompt_of)
                    .collect::<Vec<_>>()
                    .join("\n");
                assert!(!final_text.to_ascii_lowercase().contains("previous reply"));
                if note_is_echo {
                    assert!(final_text.contains("The patch was not summarized"));
                    assert!(message.description.contains("fn kept"));
                } else {
                    assert!(final_text.contains("FILE_NOTE kept.rs"));
                    assert!(message.description.contains("FILE_NOTE kept.rs"));
                }
            }
            server.await?.map_err(|error| error.to_string())?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn unreadable_file_note_still_reaches_the_commit_message()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = Ollama::new(&format!("http://{}", listener.local_addr()?))?;
        let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = prompts.clone();
        let server = tokio::spawn(serve_commit(listener, prompts, |json| {
            if requires(json, "files") {
                return (
                    serde_json::json!({"files":[{"path":"other.rs","note":"wrong path"}]})
                        .to_string(),
                    true,
                    "stop",
                );
            }
            (
                serde_json::json!({
                    "summary": "Keep the staged parser",
                    "description": "The visible hunk was preserved for kept.rs."
                })
                .to_string(),
                true,
                "stop",
            )
        }));
        tokio::time::timeout(
            Duration::from_secs(5),
            client.commit_message(
                "selected-local-model",
                &[staged("kept.rs", "@@ -1 +1 @@ fn kept\n-a\n+b\n")],
                |_| {},
            ),
        )
        .await??;
        {
            let prompts = recorded.lock().map_err(|_| "prompt lock")?.clone();
            let final_text = prompts
                .iter()
                .filter(|json| requires(json, "description"))
                .map(prompt_of)
                .collect::<Vec<_>>()
                .join("\n");
            assert!(final_text.contains("kept.rs"));
            assert!(final_text.contains("fn kept"));
            assert!(final_text.contains("The patch was not summarized"));
        }
        server.await?.map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn vague_commit_message_is_rewritten_from_the_same_notes()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = Ollama::new(&format!("http://{}", listener.local_addr()?))?;
        let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = prompts.clone();
        let finals = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let final_count = finals.clone();
        let server = tokio::spawn(serve_commit(listener, prompts, move |json| {
            if requires(json, "files") {
                return (
                    serde_json::json!({"files":[
                        {"path":"crates/ui/panel.rs","note":"FILE_NOTE panel"},
                        {"path":"crates/vcs/context.rs","note":"FILE_NOTE context"}
                    ]})
                    .to_string(),
                    true,
                    "stop",
                );
            }
            if final_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                return (
                    serde_json::json!({"summary":"Update files","description":"Update files"})
                        .to_string(),
                    true,
                    "stop",
                );
            }
            (
                serde_json::json!({
                    "summary": "Refresh Git commit text",
                    "description": "- Show panel state in crates/ui\n- Read staged files in crates/vcs"
                })
                .to_string(),
                true,
                "stop",
            )
        }));
        let message = tokio::time::timeout(
            Duration::from_secs(5),
            client.commit_message(
                "selected-local-model",
                &[
                    staged("crates/ui/panel.rs", "+panel\n"),
                    staged("crates/vcs/context.rs", "+context\n"),
                ],
                |_| {},
            ),
        )
        .await??;
        assert_eq!(message.summary, "Refresh Git commit text");
        assert!(message.description.contains("crates/vcs"));
        {
            let prompts = recorded.lock().map_err(|_| "prompt lock")?.clone();
            let final_prompts = prompts
                .iter()
                .filter(|json| requires(json, "description"))
                .map(prompt_of)
                .collect::<Vec<_>>();
            let retry = final_prompts.get(1).ok_or("missing rewritten prompt")?;
            assert_eq!(final_prompts.len(), 2);
            assert!(retry.contains("missed these areas"));
            assert!(retry.contains("crates/vcs"));
        }
        assert_eq!(finals.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.await?.map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn binary_file_does_not_request_a_file_summary() -> Result<(), Box<dyn std::error::Error>>
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = Ollama::new(&format!("http://{}", listener.local_addr()?))?;
        let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = prompts.clone();
        let server = tokio::spawn(serve_commit(listener, prompts, |_| {
            (
                serde_json::json!({
                    "summary": "Add the logo",
                    "description": "Add binary assets/logo.png."
                })
                .to_string(),
                true,
                "stop",
            )
        }));
        let mut logo = staged("assets/logo.png", "");
        logo.binary = true;
        logo.status = "added".into();
        logo.insertions = 0;
        logo.deletions = 0;
        logo.hunks.clear();
        tokio::time::timeout(
            Duration::from_secs(5),
            client.commit_message("selected-local-model", &[logo], |_| {}),
        )
        .await??;
        {
            let prompts = recorded.lock().map_err(|_| "prompt lock")?.clone();
            let only = prompts.first().ok_or("missing commit prompt")?;
            assert_eq!(prompts.len(), 1);
            assert!(!requires(only, "files"));
            assert!(prompt_of(only).contains("assets/logo.png"));
            assert!(prompt_of(only).contains("binary"));
        }
        server.await?.map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn large_commit_reduces_file_notes_before_the_title()
    -> Result<(), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = Ollama::new(&format!("http://{}", listener.local_addr()?))?;
        let prompts = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let recorded = prompts.clone();
        let groups = std::sync::Arc::new(std::sync::atomic::AtomicU32::new(0));
        let group_count = groups.clone();
        let server = tokio::spawn(serve_commit(listener, prompts, move |json| {
            if requires(json, "files") {
                let notes = prompt_of(json)
                    .lines()
                    .filter_map(|line| line.trim().strip_prefix("path: "))
                    .filter(|path| path.starts_with("src/file"))
                    .map(|path| {
                        serde_json::json!({"path": path, "note": format!("FILE_NOTE {path}")})
                    })
                    .collect::<Vec<_>>();
                return (
                    serde_json::json!({"files": notes}).to_string(),
                    true,
                    "stop",
                );
            }
            if !requires(json, "description") {
                let index = group_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
                return (
                    serde_json::json!({"summary": format!("GROUP_SUMMARY {index}")}).to_string(),
                    true,
                    "stop",
                );
            }
            (
                serde_json::json!({
                    "summary": "Update the src file group",
                    "description": "Apply the grouped src changes."
                })
                .to_string(),
                true,
                "stop",
            )
        }));
        let files = (0..25)
            .map(|index| staged(&format!("src/file{index:02}.rs"), "+a ONLY_PATCH_TOKEN\n"))
            .collect::<Vec<_>>();
        tokio::time::timeout(
            Duration::from_secs(5),
            client.commit_message("selected-local-model", &files, |_| {}),
        )
        .await??;
        {
            let prompts = recorded.lock().map_err(|_| "prompt lock")?.clone();
            let map_text = prompts
                .iter()
                .filter(|json| requires(json, "files"))
                .map(prompt_of)
                .collect::<Vec<_>>()
                .join("\n");
            let final_text = prompts
                .iter()
                .filter(|json| requires(json, "description"))
                .map(prompt_of)
                .collect::<Vec<_>>()
                .join("\n");
            assert_eq!(
                prompts
                    .iter()
                    .filter(|json| requires(json, "files"))
                    .count(),
                7
            );
            assert!(map_text.contains("ONLY_PATCH_TOKEN"));
            assert!(map_text.contains("path: src/file00.rs"));
            assert!(map_text.contains("path: src/file24.rs"));
            assert!(final_text.contains("GROUP_SUMMARY 1"));
            assert!(final_text.contains("GROUP_SUMMARY 2"));
            assert!(!final_text.contains("ONLY_PATCH_TOKEN"));
            assert!(!final_text.contains("FILE_NOTE"));
        }
        assert_eq!(groups.load(std::sync::atomic::Ordering::SeqCst), 2);
        server.await?.map_err(|error| error.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn abort_closes_obsolete_http_request() -> Result<(), Box<dyn std::error::Error>> {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let endpoint = format!("http://{}", listener.local_addr()?);
        let (received, ready) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let mut bytes = [0; 8192];
            let body = request_body(&mut socket).await?;
            assert!(String::from_utf8_lossy(&body).contains("fim_prefix"));
            let _ = received.send(()); // Test observation may time out.
            let count =
                tokio::time::timeout(Duration::from_secs(2), socket.read(&mut bytes)).await??;
            assert_eq!(
                count, 0,
                "aborting a generation must close its HTTP connection"
            );
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        });
        let generation = Generation::start(
            &Handle::current(),
            Ollama::new(&endpoint)?,
            DEFAULT_MODEL.into(),
            fim::prompt("print", ";"),
            Duration::ZERO,
            || {},
        );
        tokio::time::timeout(Duration::from_secs(2), ready).await??;
        drop(generation); // Same operation used by typing, navigation, switches and editor drop.
        server.await?.map_err(|e| e.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn debounce_replacement_only_sends_latest_fim() -> Result<(), Box<dyn std::error::Error>>
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let client = Ollama::new(&format!("http://{}", listener.local_addr()?))?;
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await?;
            let body = request_body(&mut socket).await?;
            let json: serde_json::Value = serde_json::from_slice(&body)?;
            assert_eq!(json["prompt"], fim::prompt("latest", ";"));
            assert_eq!(json["raw"], true);
            assert_eq!(json["stream"], false);
            assert_eq!(json["options"]["stop"][0], "\n\n");
            let body = r#"{"response":"result"}"#;
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await?;
            assert!(
                tokio::time::timeout(Duration::from_millis(80), listener.accept())
                    .await
                    .is_err()
            );
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        });
        let obsolete = Generation::start(
            &Handle::current(),
            client.clone(),
            DEFAULT_MODEL.into(),
            fim::prompt("obsolete", ""),
            Duration::from_millis(50),
            || {},
        );
        drop(obsolete);
        let (ready, finished) = oneshot::channel();
        let mut latest = Generation::start(
            &Handle::current(),
            client,
            DEFAULT_MODEL.into(),
            fim::prompt("latest", ";"),
            Duration::from_millis(10),
            move || {
                let _ = ready.send(());
            },
        );
        tokio::time::timeout(Duration::from_secs(2), finished).await??;
        assert_eq!(latest.poll(), Some(Ok(Some("result".into()))));
        server.await?.map_err(|e| e.to_string())?;
        Ok(())
    }

    #[tokio::test]
    async fn pull_handles_split_records_and_missing_success()
    -> Result<(), Box<dyn std::error::Error>> {
        for success in [true, false] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let endpoint = format!("http://{}", listener.local_addr()?);
            let server = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await?;
                let mut request = [0; 8192];
                let count = socket.read(&mut request).await?;
                assert!(count > 0);
                let mut body =
                    "{\"status\":\"pulling\",\"total\":100,\"completed\":37}\n".to_owned();
                if success {
                    body.push_str("{\"status\":\"success\"}");
                }
                socket
                    .write_all(
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            body.len()
                        )
                        .as_bytes(),
                    )
                    .await?;
                for byte in body.bytes() {
                    socket.write_all(&[byte]).await?;
                    tokio::task::yield_now().await;
                }
                Ok::<_, std::io::Error>(())
            });
            let mut records = Vec::new();
            let result = Ollama::new(&endpoint)?
                .pull_model(DEFAULT_MODEL, |p| records.push(p))
                .await;
            assert_eq!(result.is_ok(), success);
            assert_eq!(records.first().map(|p| p.completed), Some(37));
            server.await??;
        }
        Ok(())
    }
}
