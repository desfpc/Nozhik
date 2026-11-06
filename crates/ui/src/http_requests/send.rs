//! Background HTTP calls for the request builder. The UI thread only submits and polls.
#![allow(clippy::redundant_pub_crate)]

use std::{
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const BODY_LIMIT: usize = 512 * 1024;

/// One completed call. A transport failure leaves [`Outcome::status_code`] empty.
#[derive(Clone, Debug)]
pub(crate) struct Outcome {
    /// HTTP status, when the server answered.
    pub status_code: Option<u16>,
    /// Status code and reason, or empty when the call did not reach a server.
    pub status_line: String,
    /// Response headers, one per line, capped at 64 lines.
    pub headers: String,
    /// Response body, capped at 512 KiB.
    pub body: String,
    /// Wall time from submit to the finished read.
    pub elapsed_ms: u128,
    /// Present when the call failed before a response body was read.
    pub transport_error: Option<String>,
}

impl Outcome {
    pub(crate) fn transport(message: impl Into<String>) -> Self {
        Self {
            status_code: None,
            status_line: String::new(),
            headers: String::new(),
            body: String::new(),
            elapsed_ms: 0,
            transport_error: Some(message.into()),
        }
    }
}

pub(crate) struct Outgoing {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
}

enum Command {
    Call {
        outgoing: Outgoing,
        reply: mpsc::Sender<Outcome>,
    },
}

#[derive(Default)]
pub(crate) struct Hub {
    tx: Option<mpsc::Sender<Command>>,
}

impl Hub {
    pub(crate) fn submit(&mut self, outgoing: Outgoing) -> Result<mpsc::Receiver<Outcome>, String> {
        self.start()?;
        let (reply, receiver) = mpsc::channel();
        let sender = self
            .tx
            .as_ref()
            .ok_or_else(|| "HTTP worker is unavailable".to_string())?;
        if sender.send(Command::Call { outgoing, reply }).is_err() {
            self.tx = None;
            return Err("HTTP worker stopped".into());
        }
        Ok(receiver)
    }

    fn start(&mut self) -> Result<(), String> {
        if self.tx.is_some() {
            return Ok(());
        }
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("nozhik-http".into())
            .spawn(move || worker(receiver))
            .map_err(|error| error.to_string())?;
        self.tx = Some(sender);
        Ok(())
    }
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "The worker thread owns the channel end and drops it when the sender disconnects"
)]
fn worker(receiver: mpsc::Receiver<Command>) {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            let message = format!("HTTP runtime: {error}");
            while let Ok(command) = receiver.recv() {
                let Command::Call { reply, .. } = command;
                let _sent = reply.send(Outcome::transport(message.clone()));
            }
            return;
        }
    };
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::limited(10))
        .user_agent("nozhik")
        .build()
    {
        Ok(client) => client,
        Err(error) => {
            let message = format!("HTTP client: {error}");
            while let Ok(command) = receiver.recv() {
                let Command::Call { reply, .. } = command;
                let _sent = reply.send(Outcome::transport(message.clone()));
            }
            return;
        }
    };
    while let Ok(command) = receiver.recv() {
        let Command::Call { outgoing, reply } = command;
        let outcome = runtime.block_on(perform(&client, outgoing));
        let _sent = reply.send(outcome);
    }
}

async fn perform(client: &reqwest::Client, outgoing: Outgoing) -> Outcome {
    let started = Instant::now();
    let method = match reqwest::Method::from_bytes(outgoing.method.as_bytes()) {
        Ok(method) => method,
        Err(error) => return Outcome::transport(error.to_string()),
    };
    let mut request = client.request(method, &outgoing.url);
    for (name, value) in &outgoing.headers {
        let Ok(name) = reqwest::header::HeaderName::try_from(name.as_str()) else {
            return Outcome::transport(format!("Invalid header name {name}"));
        };
        let Ok(value) = reqwest::header::HeaderValue::try_from(value.as_str()) else {
            return Outcome::transport(format!("Header {name} must be visible ASCII"));
        };
        request = request.header(name, value);
    }
    if let Some(body) = outgoing.body.clone() {
        request = request.body(body);
    }
    let response = match request.send().await {
        Ok(response) => response,
        Err(error) => return timed(started, Outcome::transport(error.to_string())),
    };
    let status = response.status();
    let mut header_lines = Vec::new();
    for (name, value) in response.headers() {
        if header_lines.len() == 64 {
            header_lines.push("…".into());
            break;
        }
        let text = value
            .to_str()
            .unwrap_or("")
            .chars()
            .take(4096)
            .collect::<String>();
        header_lines.push(format!("{name}: {text}"));
    }
    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("")
        .to_string();
    let (body, truncated) = match read_body(response).await {
        Ok(result) => result,
        Err(error) => return timed(started, Outcome::transport(error)),
    };
    let mut body = if content_type.contains("json") {
        nozhik_language_support::json::render(&body, Some("  ")).unwrap_or(body)
    } else {
        body
    };
    if truncated {
        body.push_str("\n\n… response truncated at 512 KiB");
    }
    let reason = status.canonical_reason().unwrap_or("");
    let status_line = if reason.is_empty() {
        status.as_u16().to_string()
    } else {
        format!("{} {reason}", status.as_u16())
    };
    timed(
        started,
        Outcome {
            status_code: Some(status.as_u16()),
            status_line,
            headers: header_lines.join("\n"),
            body,
            elapsed_ms: 0,
            transport_error: None,
        },
    )
}

async fn read_body(mut response: reqwest::Response) -> Result<(String, bool), String> {
    let mut collected = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
        if collected.len() >= BODY_LIMIT {
            truncated = true;
            break;
        }
        let room = BODY_LIMIT - collected.len();
        if chunk.len() > room {
            collected.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        collected.extend_from_slice(&chunk);
    }
    Ok((String::from_utf8_lossy(&collected).into_owned(), truncated))
}

fn timed(started: Instant, mut outcome: Outcome) -> Outcome {
    outcome.elapsed_ms = started.elapsed().as_millis();
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        io::{ErrorKind, Read, Write},
        net::TcpListener,
    };

    #[test]
    fn posts_json_to_a_local_listener() -> Result<(), String> {
        let listener = TcpListener::bind("127.0.0.1:0").map_err(|error| error.to_string())?;
        let address = listener.local_addr().map_err(|error| error.to_string())?;
        let server = thread::Builder::new()
            .name("http-requests-test".into())
            .spawn(move || serve(listener))
            .map_err(|error| error.to_string())?;
        let mut hub = Hub::default();
        let receiver = hub.submit(Outgoing {
            method: "POST".into(),
            url: format!("http://{address}/items"),
            headers: vec![
                ("Content-Type".into(), "application/json".into()),
                ("X-Test".into(), "yes".into()),
            ],
            body: Some(br#"{"name":"Ada"}"#.to_vec()),
        })?;
        let outcome = receiver
            .recv_timeout(Duration::from_secs(8))
            .map_err(|error| error.to_string())?;
        let captured = server
            .join()
            .map_err(|_| "server thread panicked".to_string())?;
        assert_eq!(outcome.transport_error, None, "{outcome:?}");
        assert_eq!(outcome.status_code, Some(201));
        assert!(outcome.body.contains("\"ok\": true") || outcome.body.contains("\"ok\":true"));
        assert!(captured.contains("POST /items"), "{captured}");
        assert!(
            captured.to_ascii_lowercase().contains("x-test: yes"),
            "{captured}"
        );
        assert!(captured.contains("Ada"), "{captured}");
        Ok(())
    }

    #[allow(
        clippy::needless_pass_by_value,
        reason = "The test server owns the listener until the request is read"
    )]
    fn serve(listener: TcpListener) -> String {
        let Ok((mut stream, _)) = listener.accept() else {
            return String::new();
        };
        let _timeout = stream.set_read_timeout(Some(Duration::from_millis(200)));
        let mut collected = Vec::new();
        let mut buffer = [0_u8; 2048];
        let deadline = Instant::now() + Duration::from_secs(3);
        while Instant::now() < deadline {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    collected.extend_from_slice(&buffer[..count]);
                    if collected.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(error)
                    if error.kind() == ErrorKind::WouldBlock
                        || error.kind() == ErrorKind::TimedOut =>
                {
                    if collected.windows(4).any(|window| window == b"\r\n\r\n") {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let payload = b"{\"ok\":true}";
        let header = format!(
            "HTTP/1.1 201 Created\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            payload.len()
        );
        let _written = stream.write_all(header.as_bytes());
        let _written = stream.write_all(payload);
        String::from_utf8_lossy(&collected).into_owned()
    }
}
