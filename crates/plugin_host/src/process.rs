//! Bounded JSON-RPC over child stdio; child failure never unwinds the IDE.
use crate::{Error, Result, manifest::PluginManifest};
use serde_json::{Value, json};
use std::{path::Path, process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader, Take},
    process::{Child, ChildStdin, ChildStdout, Command},
};

pub struct Session {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    sequence: u64,
}
impl Session {
    pub async fn start(root: &Path, manifest: &PluginManifest) -> Result<Self> {
        let executable = manifest
            .executable
            .as_ref()
            .ok_or_else(|| Error::Invalid("no executable".into()))?;
        let root = root.canonicalize()?;
        let path = root.join(executable).canonicalize()?;
        if !path.starts_with(&root) {
            return Err(Error::Invalid("executable escapes plugin directory".into()));
        }
        let mut child = Command::new(path)
            .args(&manifest.args)
            .current_dir(root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Invalid("missing plugin stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Invalid("missing plugin stdout".into()))?;
        let mut session = Self {
            child,
            stdin,
            stdout: BufReader::new(stdout),
            sequence: 0,
        };
        session.call("initialize", json!({"ide_version": env!("CARGO_PKG_VERSION"), "plugin_id": manifest.id, "configuration": manifest.contributes.configuration})).await?;
        Ok(session)
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Result<()> {
        self.sequence += 1;
        let id = self.sequence;
        let request = serde_json::to_vec(
            &json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}),
        )?;
        tokio::time::timeout(Duration::from_secs(3), async {
            self.stdin.write_all(&request).await?;
            self.stdin.write_all(b"\n").await?;
            self.stdin.flush().await?;
            let mut response = Vec::new();
            // Limit the allocation even if a hostile child never sends a newline.
            let mut limited: Take<&mut BufReader<ChildStdout>> = (&mut self.stdout).take(65_537);
            limited.read_until(b'\n', &mut response).await?;
            if response.len() > 65_536 || !response.ends_with(b"\n") {
                return Err(Error::Invalid(
                    "plugin response is missing, oversized or unterminated".into(),
                ));
            }
            let value: Value = serde_json::from_slice(&response)?;
            if value.get("jsonrpc") != Some(&json!("2.0"))
                || value.get("id") != Some(&json!(id))
                || value.get("error").is_some()
                || value.get("result").is_none()
            {
                return Err(Error::Invalid(format!("plugin RPC failed: {value}")));
            }
            Ok(())
        })
        .await
        .map_err(|_| Error::Invalid("plugin RPC timed out after 3 seconds".into()))?
    }

    pub fn exited(&mut self) -> Result<bool> {
        Ok(self.child.try_wait()?.is_some())
    }

    pub async fn stop(mut self) {
        // Killing the direct child cancels even an unresponsive plugin; wait reaps it.
        if let Err(error) = self.child.kill().await {
            eprintln!("Plugin shutdown: {error}");
        }
    }
}
