//! Bounded Content-Length framing and multiplexed JSON-RPC over stdio.
use crate::{Error, Result};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
use tokio::{
    io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader},
    process::{Child, Command},
    sync::{Mutex, mpsc, oneshot},
};

const MAX_BODY: usize = 16 * 1024 * 1024;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value>>>>>;

/// Reads one framed JSON packet, allowing split headers and arbitrary JSON UTF-8.
/// # Errors
/// Rejects oversized, duplicate/missing length, malformed JSON, and truncated frames.
pub async fn read_packet<R: AsyncBufRead + Unpin>(reader: &mut R) -> Result<Value> {
    let mut length = None;
    let mut total = 0;
    loop {
        let mut line = Vec::new();
        loop {
            let bytes = reader.fill_buf().await?;
            if bytes.is_empty() {
                return Err(Error::Protocol("EOF in headers".into()));
            }
            let count = bytes
                .iter()
                .position(|b| *b == b'\n')
                .map_or(bytes.len(), |n| n + 1);
            line.extend_from_slice(&bytes[..count]);
            reader.consume(count);
            total += count;
            if total > 8192 {
                return Err(Error::Protocol("headers too large".into()));
            }
            if line.ends_with(b"\n") {
                break;
            }
        }
        if line == b"\r\n" {
            break;
        }
        let line = std::str::from_utf8(&line).map_err(|e| Error::Protocol(e.to_string()))?;
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| Error::Protocol("invalid header".into()))?;
        if name.eq_ignore_ascii_case("Content-Length") {
            if length.is_some() {
                return Err(Error::Protocol("duplicate Content-Length".into()));
            }
            length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .map_err(|e| Error::Protocol(e.to_string()))?,
            );
        }
    }
    let length = length
        .filter(|n| *n <= MAX_BODY)
        .ok_or_else(|| Error::Protocol("missing/oversized Content-Length".into()))?;
    let mut body = vec![0; length];
    reader.read_exact(&mut body).await?;
    Ok(serde_json::from_slice(&body)?)
}
/// Writes a packet; length counts bytes, not characters.
/// # Errors
/// Returns serialization or I/O failures.
pub async fn write_packet<W: AsyncWrite + Unpin>(writer: &mut W, packet: &Value) -> Result<()> {
    let body = serde_json::to_vec(packet)?;
    writer
        .write_all(format!("Content-Length: {}\r\n\r\n", body.len()).as_bytes())
        .await?;
    writer.write_all(&body).await?;
    writer.flush().await?;
    Ok(())
}
/// Concurrent requests share one reader; notifications never consume responses.
pub struct Client {
    writer: Arc<Mutex<tokio::process::ChildStdin>>,
    pending: Pending,
    next: AtomicU64,
    reader: tokio::task::JoinHandle<()>,
    child: Child,
}
impl Client {
    /// Spawns a kill-on-drop child and routes responses by numeric request ID.
    /// # Errors
    /// Returns spawn/stdio failures.
    pub fn spawn(command: &mut Command) -> Result<(Self, mpsc::UnboundedReceiver<Value>)> {
        let mut child = command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .kill_on_drop(true)
            .spawn()?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Protocol("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Protocol("no stdout".into()))?;
        let writer = Arc::new(Mutex::new(stdin));
        let pending: Pending = Arc::default();
        let (tx, rx) = mpsc::unbounded_channel();
        let route = pending.clone();
        let output = writer.clone();
        let reader = tokio::spawn(async move {
            let mut input = BufReader::new(stdout);
            while let Ok(packet) = read_packet(&mut input).await {
                if packet.get("method").is_some() {
                    if let Some(id) = packet.get("id") {
                        let reply = match packet["method"].as_str() {
                            Some("workspace/configuration") => {
                                json!({"jsonrpc":"2.0","id":id,"result":packet["params"]["items"].as_array().map(|a| vec![Value::Null;a.len()]).unwrap_or_default()})
                            }
                            Some("window/workDoneProgress/create") => {
                                json!({"jsonrpc":"2.0","id":id,"result":null})
                            }
                            _ => {
                                json!({"jsonrpc":"2.0","id":id,"error":{"code":-32601,"message":"Method not supported"}})
                            }
                        };
                        if write_packet(&mut *output.lock().await, &reply)
                            .await
                            .is_err()
                        {
                            break;
                        }
                    } else if tx.send(packet).is_err() {
                        break;
                    }
                } else if let Some(id) = packet["id"].as_u64()
                    && let Some(sender) = route.lock().await.remove(&id)
                {
                    let result = if packet.get("error").is_some() {
                        Err(Error::Protocol(packet["error"].to_string()))
                    } else {
                        Ok(packet["result"].clone())
                    };
                    let _delivered = sender.send(result);
                }
            }
            route.lock().await.clear();
        });
        Ok((
            Self {
                writer,
                pending,
                next: AtomicU64::new(1),
                reader,
                child,
            },
            rx,
        ))
    }
    /// Sends a request with a five-second deadline and removes timed-out entries.
    /// # Errors
    /// Returns transport, remote, timeout, or disconnect failures.
    pub async fn request(&self, method: &str, params: Value) -> Result<Value> {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(id, tx);
        let result = tokio::time::timeout(std::time::Duration::from_secs(5), async {
            write_packet(
                &mut *self.writer.lock().await,
                &json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
            )
            .await?;
            rx.await.map_err(|e| Error::Protocol(e.to_string()))?
        })
        .await
        .map_err(|_| Error::Protocol(format!("{method} timed out")))
        .and_then(|v| v);
        self.pending.lock().await.remove(&id);
        result
    }
    /// Sends a notification.
    /// # Errors
    /// Returns transport failures.
    pub async fn notify(&self, method: &str, params: Value) -> Result<()> {
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            write_packet(
                &mut *self.writer.lock().await,
                &json!({"jsonrpc":"2.0","method":method,"params":params}),
            ),
        )
        .await
        .map_err(|_| Error::Protocol(format!("{method} write timed out")))?
    }
    /// Graceful shutdown with bounded exit wait and kill/reap fallback.
    pub async fn shutdown(&mut self) {
        let _shutdown = self.request("shutdown", Value::Null).await;
        let _exit = self.notify("exit", Value::Null).await;
        if tokio::time::timeout(std::time::Duration::from_secs(1), self.child.wait())
            .await
            .is_err()
        {
            let _killed = self.child.kill().await;
        }
        self.reader.abort();
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.reader.abort();
    }
}
