//! Behavioral coverage for PHP syntax, UTF-16, framing and a real stdio mock.
use nozhik_language_support::{
    byte_offset, byte_range, completion,
    highlighter::Highlighter,
    lsp::{
        client::{read_packet, write_packet},
        manager::LspManager,
    },
    lsp_types::{CompletionResponse, Position, PublishDiagnosticsParams, Range, Url},
    position,
};
use serde_json::json;
use tokio::io::BufReader;

type TestResult = Result<(), Box<dyn std::error::Error>>;

#[test]
fn php_queries_and_incremental_unicode_replacement() -> TestResult {
    let source = "<?php\n// note\nfunction hello(string $имя): string { return \"hi 🦀\"; }";
    let mut highlighter = Highlighter::php()?;
    let spans = highlighter.highlight(source)?;
    for token in [
        "keyword",
        "comment",
        "function",
        "variable",
        "string",
        "type.builtin",
    ] {
        assert!(spans.iter().any(|s| s.token == token), "missing {token}");
    }
    let changed = source.replace("hi 🦀", "Привет 🦀🦀");
    let incremental = highlighter.highlight(&changed)?;
    let fresh = Highlighter::php()?.highlight(&changed)?;
    assert_eq!(
        incremental
            .iter()
            .map(|s| (&s.range, &s.token))
            .collect::<Vec<_>>(),
        fresh
            .iter()
            .map(|s| (&s.range, &s.token))
            .collect::<Vec<_>>()
    );
    assert!(
        incremental
            .iter()
            .all(|s| changed.is_char_boundary(s.range.start)
                && changed.is_char_boundary(s.range.end))
    );
    Ok(())
}
#[test]
fn utf16_roundtrip_crlf_and_completion_edits() -> TestResult {
    let source = "🦀Ж\r\n$fo";
    for byte in source
        .char_indices()
        .map(|(i, _)| i)
        .chain([source.len()])
        .filter(|i| !source[..*i].ends_with('\r'))
    {
        assert_eq!(byte_offset(source, position(source, byte)), Some(byte));
    }
    assert_eq!(byte_offset(source, Position::new(0, 1)), None);
    assert_eq!(byte_offset("x\r", Position::new(0, 2)), Some(2));
    assert_eq!(byte_offset("x\r\r\n", Position::new(0, 2)), Some(2));
    assert!(byte_range(source, Range::new(Position::new(1, 3), Position::new(1, 0))).is_none());
    let item = serde_json::from_value(
        json!({"label":"$foo","textEdit":{"range":{"start":{"line":1,"character":0},"end":{"line":1,"character":3}},"newText":"$foo"},"additionalTextEdits":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":0}},"newText":"// import\n"}]}),
    )?;
    let edits = completion::edits(&item, source, source.len()).ok_or("no edits")?;
    assert_eq!(&source[edits[0].0.clone()], "$fo");
    assert_eq!(edits.len(), 2);
    assert_eq!(completion::filter(vec![item], "$f").len(), 1);
    Ok(())
}
#[tokio::test]
async fn framing_unicode_split_packets_and_invalid_headers() -> TestResult {
    let packet = json!({"jsonrpc":"2.0","id":42,"result":"Привет 🦀"});
    let mut bytes = Vec::new();
    write_packet(&mut bytes, &packet).await?;
    write_packet(&mut bytes, &json!({"id":43,"result":null})).await?;
    let mut reader = BufReader::with_capacity(3, bytes.as_slice());
    assert_eq!(read_packet(&mut reader).await?, packet);
    assert_eq!(read_packet(&mut reader).await?["id"], 43);
    for invalid in [
        "Content-Length: 4\r\nContent-Length: 4\r\n\r\nnull",
        "Other: 4\r\n\r\nnull",
        "Content-Length: 999999999\r\n\r\n",
        "Content-Length: 5\r\n\r\n{}",
    ] {
        assert!(
            read_packet(&mut BufReader::new(invalid.as_bytes()))
                .await
                .is_err()
        );
    }
    Ok(())
}
#[tokio::test]
async fn process_initialize_completion_virtual_open_diagnostics_and_shutdown() -> TestResult {
    let dir = tempfile::tempdir()?;
    let binary = dir
        .path()
        .join(format!("mock-lsp{}", std::env::consts::EXE_SUFFIX));
    let status =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/server.rs"))
            .arg("-o")
            .arg(&binary)
            .output()?;
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    tokio::time::timeout(std::time::Duration::from_secs(15), async {
        let (mut manager, mut events) =
            LspManager::start(&mut tokio::process::Command::new(binary), dir.path()).await?;
        let uri = Url::from_file_path(dir.path().join("virtual.php")).map_err(|()| "URI")?;
        manager.sync(&uri, "<?php nope()").await?;
        let diagnostic: PublishDiagnosticsParams =
            serde_json::from_value(events.recv().await.ok_or("closed")?["params"].clone())?;
        assert_eq!(diagnostic.uri, uri);
        assert_eq!(diagnostic.diagnostics[0].message, "mock error");
        let params = json!({"textDocument":{"uri":uri},"position":{"line":0,"character":12}});
        let (completion, hover) = tokio::join!(
            manager
                .client
                .request("textDocument/completion", params.clone()),
            manager.client.request("textDocument/hover", params)
        );
        let completion: CompletionResponse = serde_json::from_value(completion?)?;
        assert!(matches!(completion,CompletionResponse::Array(items) if items[0].label == "hello"));
        assert_eq!(hover?["contents"]["value"], "**hello**");
        manager.sync(&uri, "<?php hello()").await?;
        assert_eq!(manager.version(&uri), Some(2));
        manager.close(&uri).await?;
        assert_eq!(manager.version(&uri), None);
        manager.shutdown().await;
        Ok::<(), Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn workers_debounce_revisions_close_and_graceful_exit() -> TestResult {
    use nozhik_language_support::{Command, Event, LanguageService, ServerCommand};
    let dir = tempfile::tempdir()?;
    let binary = dir
        .path()
        .join(format!("mock-lsp{}", std::env::consts::EXE_SUFFIX));
    let output =
        std::process::Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
            .arg(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/server.rs"))
            .arg("-o")
            .arg(&binary)
            .output()?;
    assert!(output.status.success());
    let log = dir.path().join("messages.jsonl");
    let service = LanguageService::with_server(
        dir.path().into(),
        Some(ServerCommand {
            program: binary,
            args: vec![log.to_string_lossy().into_owned()],
        }),
        || {},
    );
    let uri = Url::from_file_path(dir.path().join("virtual.php")).map_err(|()| "URI")?;
    assert!(service.send(Command::Sync {
        uri: uri.clone(),
        revision: 0,
        text: ropey::Rope::from_str("<?php $a")
    }));
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let mut received = false;
            for event in service.drain() {
                if let Event::Diagnostics {
                    revision, items, ..
                } = event
                {
                    assert_eq!(revision, 0);
                    assert_eq!(items.len(), 1);
                    received = true;
                }
            }
            if received {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await?;
    for revision in 1..=3 {
        assert!(service.send(Command::Sync {
            uri: uri.clone(),
            revision,
            text: ropey::Rope::from_str(&format!("<?php $a{revision}"))
        }));
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(75)).await;
    let before = std::fs::read_to_string(&log)?;
    assert!(!before.contains("textDocument/didChange"));
    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    assert!(service.send(Command::Close(uri)));
    service.shutdown();
    let packets: Vec<serde_json::Value> = std::fs::read_to_string(log)?
        .lines()
        .map(serde_json::from_str)
        .collect::<Result<_, _>>()?;
    let changes: Vec<_> = packets
        .iter()
        .filter(|p| p["method"] == "textDocument/didChange")
        .collect();
    assert_eq!(changes.len(), 1);
    assert_eq!(
        changes[0]["params"]["contentChanges"][0]["text"],
        "<?php $a3"
    );
    assert!(
        changes[0]["params"]["contentChanges"][0]
            .get("range")
            .is_some()
    );
    // Shutdown closes tracked documents even if Close was still queued at cancellation.
    assert!(
        packets
            .iter()
            .any(|p| p["method"] == "textDocument/didClose")
    );
    assert!(packets.iter().any(|p| p["method"] == "shutdown"));
    assert!(packets.iter().any(|p| p["method"] == "exit"));
    Ok(())
}
