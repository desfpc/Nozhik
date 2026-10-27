//! Asynchronous Ollama HTTP transport and newline-delimited pull progress.
use serde::Deserialize;
use std::time::Duration;

/// Transport, server or protocol failure.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// HTTP connection/status/body error.
    #[error("{0}")]
    Http(#[from] reqwest::Error),
    /// Malformed progress record.
    #[error("{0}")]
    Json(#[from] serde_json::Error),
    /// Ollama reported a failure or an incomplete stream.
    #[error("{0}")]
    Server(String),
}
/// A pull stream record; counters describe the current layer, in bytes.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct PullProgress {
    /// Human-readable phase, including the terminal `success`.
    #[serde(default)]
    pub status: String,
    /// Layer digest, when downloading a layer.
    #[serde(default)]
    pub digest: String,
    /// Layer byte count.
    #[serde(default)]
    pub total: u64,
    /// Bytes already downloaded for this layer.
    #[serde(default)]
    pub completed: u64,
    /// Server-side error.
    pub error: Option<String>,
}
impl PullProgress {
    /// Parses one NDJSON record, propagating explicit server failures.
    /// # Errors
    /// Returns malformed JSON or Ollama's error.
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        let record: Self = serde_json::from_slice(bytes)?;
        if let Some(error) = &record.error {
            return Err(Error::Server(error.clone()));
        }
        Ok(record)
    }
    /// Fraction of the current layer, clamped to 0–1.
    #[must_use]
    #[allow(
        clippy::cast_precision_loss,
        reason = "Progress fractions only need display precision"
    )]
    pub fn fraction(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            (self.completed as f32 / self.total as f32).clamp(0.0, 1.0)
        }
    }
}
/// Reusable HTTP connection pool. Construction does no network I/O.
#[derive(Clone)]
pub struct Ollama {
    client: reqwest::Client,
    endpoint: String,
}
impl Ollama {
    /// Creates a client; generation/tags have individual deadlines, pulls a long deadline.
    /// # Errors
    /// Returns a TLS/HTTP initialization error.
    pub fn new(endpoint: &str) -> Result<Self, Error> {
        Ok(Self {
            client: reqwest::Client::builder()
                .connect_timeout(Duration::from_millis(1000))
                .build()?,
            endpoint: endpoint.trim_end_matches('/').into(),
        })
    }
    /// Generates raw FIM text, with a 60 s total deadline for cold model loading and bounded output.
    /// # Errors
    /// Returns transport, HTTP status or server errors.
    pub async fn generate(&self, model: &str, prompt: &str) -> Result<String, Error> {
        #[derive(Deserialize)]
        struct Reply {
            response: String,
        }
        let reply: Reply = self
            .client
            .post(format!("{}/api/generate", self.endpoint))
            .timeout(Duration::from_secs(60))
            .json(
                &serde_json::json!({"model":model,"prompt":prompt,"raw":true,"stream":false,
                "options":{"num_predict":64,"temperature":0.2,"stop":["\n\n","<|fim_pad|>","<|file_separator|>"]}}),
            )
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(reply.response)
    }
    /// Preloads an installed model, or unloads it with `keep_alive=0` and no prompt.
    /// No download is performed by this endpoint.
    /// # Errors
    /// Returns transport, status, JSON or server errors.
    pub async fn residency(&self, model: &str, load: bool) -> Result<(), Error> {
        let value: serde_json::Value = self.client.post(format!("{}/api/generate", self.endpoint))
            .timeout(Duration::from_secs(if load { 60 } else { 5 }))
            .json(&serde_json::json!({"model":model,"stream":false,"keep_alive":if load { "5m" } else { "0" }}))
            .send().await?.error_for_status()?.json().await?;
        if let Some(error) = value.get("error").and_then(serde_json::Value::as_str) {
            return Err(Error::Server(error.into()));
        }
        Ok(())
    }

    /// Lists installed model names; success also confirms service connectivity.
    /// # Errors
    /// Returns transport, status or JSON errors.
    pub async fn models(&self) -> Result<Vec<String>, Error> {
        #[derive(Deserialize)]
        struct Model {
            name: String,
        }
        #[derive(Deserialize)]
        struct Tags {
            models: Vec<Model>,
        }
        let tags: Tags = self
            .client
            .get(format!("{}/api/tags", self.endpoint))
            .timeout(Duration::from_millis(1500))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(tags.models.into_iter().map(|m| m.name).collect())
    }
    /// Downloads a model explicitly, parsing records across arbitrary HTTP chunk boundaries.
    /// # Errors
    /// Returns transport/protocol errors, including an EOF without `success`.
    pub async fn pull_model(
        &self,
        model: &str,
        mut progress: impl FnMut(PullProgress),
    ) -> Result<(), Error> {
        let mut response = self
            .client
            .post(format!("{}/api/pull", self.endpoint))
            .timeout(Duration::from_secs(7200))
            .json(&serde_json::json!({"model":model,"stream":true}))
            .send()
            .await?
            .error_for_status()?;
        let mut pending = Vec::new();
        let mut success = false;
        while let Some(chunk) = response.chunk().await? {
            pending.extend_from_slice(&chunk);
            while let Some(end) = pending.iter().position(|b| *b == b'\n') {
                let line: Vec<_> = pending.drain(..=end).collect();
                if line.iter().all(u8::is_ascii_whitespace) {
                    continue;
                }
                let record = PullProgress::parse(&line)?;
                success |= record.status == "success";
                progress(record);
            }
            if pending.len() > 1024 * 1024 {
                return Err(Error::Server("Oversized pull progress record".into()));
            }
        }
        if !pending.iter().all(u8::is_ascii_whitespace) {
            let record = PullProgress::parse(&pending)?;
            success |= record.status == "success";
            progress(record);
        }
        if success {
            Ok(())
        } else {
            Err(Error::Server("Model download ended before success".into()))
        }
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn progress_and_server_errors() -> Result<(), super::Error> {
        let p =
            super::PullProgress::parse(br#"{"status":"pulling","total":1200,"completed":450}"#)?;
        assert!((p.fraction() - 0.375).abs() < f32::EPSILON);
        assert!(super::PullProgress::parse(br#"{"error":"not found"}"#).is_err());
        assert_eq!(
            super::PullProgress::parse(br#"{"status":"success"}"#)?.status,
            "success"
        );
        Ok(())
    }
}
