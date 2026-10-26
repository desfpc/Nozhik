// Standalone std-only fixture, compiled by the integration test on each OS.
use std::io::{BufRead, Read, Write};
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = std::io::stdin().lock();
    let mut output = std::io::stdout().lock();
    loop {
        let mut length = 0;
        loop {
            let mut header = String::new();
            if input.read_line(&mut header)? == 0 { return Ok(()); }
            if header == "\r\n" { break; }
            if let Some(v) = header.strip_prefix("Content-Length:") { length = v.trim().parse()?; }
        }
        let mut body = vec![0;length]; input.read_exact(&mut body)?;
        let body = String::from_utf8(body)?;
        if let Some(log) = std::env::args_os().nth(1) {
            writeln!(std::fs::OpenOptions::new().create(true).append(true).open(log)?,"{body}")?;
        }
        // Client-generated JSON uses compact numeric IDs; notifications have none.
        let id = body.split("\"id\":").nth(1).map(|s| s.split(|c:char| !c.is_ascii_digit()).next().unwrap_or("0"));
        let result = if body.contains("\"initialize\"") {
            Some("{\"capabilities\":{\"textDocumentSync\":2,\"completionProvider\":{},\"hoverProvider\":true,\"definitionProvider\":true}}".to_string())
        } else if body.contains("\"textDocument/completion\"") {
            Some("[{\"label\":\"hello\",\"kind\":2,\"insertText\":\"hello()\"}]".to_string())
        } else if body.contains("\"textDocument/hover\"") {
            Some("{\"contents\":{\"kind\":\"markdown\",\"value\":\"**hello**\"}}".to_string())
        } else if body.contains("\"shutdown\"") { Some("null".to_string()) }
        else if body.contains("\"exit\"") { return Ok(()); }
        else { None };
        if let (Some(id),Some(result)) = (id,result) {
            send(&mut output,&format!("{{\"jsonrpc\":\"2.0\",\"id\":{id},\"result\":{result}}}"))?;
        }
        if body.contains("\"textDocument/didOpen\"") || body.contains("\"textDocument/didChange\"") {
            // Extract the URI from the received document, proving virtual didOpen routing.
            let uri = body.split("\"uri\":\"").nth(1).and_then(|s| s.split('"').next()).ok_or("no uri")?;
            send(&mut output,&format!("{{\"jsonrpc\":\"2.0\",\"method\":\"textDocument/publishDiagnostics\",\"params\":{{\"uri\":\"{uri}\",\"version\":1,\"diagnostics\":[{{\"range\":{{\"start\":{{\"line\":0,\"character\":6}},\"end\":{{\"line\":0,\"character\":10}}}},\"severity\":1,\"message\":\"mock error\"}}]}}}}"))?;
        }
    }
}
fn send(output: &mut impl Write, body: &str) -> std::io::Result<()> {
    write!(output,"Content-Length: {}\r\n\r\n{}",body.len(),body)?;
    output.flush()
}
