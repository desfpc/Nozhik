//! Local Ollama FIM generation and explicit model management. No UI or filesystem I/O.
pub mod fim;
pub mod lifecycle;
pub mod ollama;

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
            assert!(request.len() < 8192);
        }
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
