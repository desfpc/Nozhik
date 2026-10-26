//! End-to-end scalability regressions. Timings are diagnostics, not FPS guarantees.
use nozhik_buffer::TextBuffer;
use nozhik_core::BufferId;
use nozhik_database::{ColumnInfo, DbValue, QueryResult};
use nozhik_ui::{DataGrid, editor_view::TextEditorView};
use std::time::{Duration, Instant};

#[test]
fn ten_megabyte_rope_edits_and_hundred_thousand_line_viewport()
-> Result<(), Box<dyn std::error::Error>> {
    let line = format!("{}\n", "x".repeat(99));
    let source = line.repeat(100_000);
    assert_eq!(source.len(), 10_000_000);
    let mut buffer = TextBuffer::load_from_str(BufferId(1), &source);
    let start = Instant::now();
    for i in 0..100 {
        let offset = 50_000 + i;
        buffer.insert(offset, "🦀")?;
        buffer.delete(offset..offset + 4)?;
    }
    assert_eq!(buffer.rope().len_bytes(), source.len());
    assert_eq!(buffer.rope(), source.as_str());
    let mut editor = TextEditorView::default();
    let ctx = egui::Context::default();
    let frame = ctx.run(input(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert!(editor.show(ui, &mut buffer, 14.0, 4, false).is_ok());
        });
    });
    assert!(
        text_shapes(&frame) < 100,
        "Only visible Rope lines should enter layout"
    );
    assert!(
        start.elapsed() < Duration::from_secs(20),
        "Scalability regression: {:?}",
        start.elapsed()
    );
    eprintln!("10 MB edits + 100k-line viewport: {:?}", start.elapsed());
    Ok(())
}
#[test]
fn grid_hundred_thousand_rows_fifty_columns_paints_only_viewport() {
    let start = Instant::now();
    let mut grid = DataGrid::new(QueryResult {
        columns: (0..50)
            .map(|i| ColumnInfo {
                name: format!("column{i}"),
                data_type: "BIGINT".into(),
            })
            .collect(),
        rows: (0..100_000)
            .map(|row| (0..50).map(|col| DbValue::Int(row * 50 + col)).collect())
            .collect(),
        ..QueryResult::default()
    });
    let ctx = egui::Context::default();
    let mut largest = 0;
    for _ in 0..10 {
        let mut frame_input = input();
        frame_input.events = vec![
            egui::Event::PointerMoved(egui::pos2(600.0, 300.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(-180.0, -6000.0),
                modifiers: egui::Modifiers::NONE,
            },
        ];
        let frame = ctx.run(frame_input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| grid.show(ui));
        });
        largest = largest.max(text_shapes(&frame));
    }
    assert!(
        largest < 250,
        "5 million cells must not enter layout: {largest}"
    );
    assert!(start.elapsed() < Duration::from_secs(20));
    eprintln!(
        "5M cells retained, {largest} visible text shapes: {:?}",
        start.elapsed()
    );
}
fn input() -> egui::RawInput {
    egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(1200.0, 700.0),
        )),
        ..egui::RawInput::default()
    }
}
fn text_shapes(frame: &egui::FullOutput) -> usize {
    frame
        .shapes
        .iter()
        .filter(|s| matches!(s.shape, egui::Shape::Text(_)))
        .count()
}

#[cfg(unix)]
#[test]
fn streaming_terminal_git_polling_and_active_lsp_buffer() -> Result<(), Box<dyn std::error::Error>>
{
    use nozhik_language_support::{
        Command as LanguageCommand, Event as LanguageEvent, LanguageService, ServerCommand,
        lsp_types::Url,
    };
    use nozhik_terminal::{
        TerminalManager,
        pty::{SessionOptions, ShellCommand},
    };
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let dir = tempfile::tempdir()?;
    let _repo = git2::Repository::init(dir.path())?;
    let mock = dir.path().join("mock-lsp");
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../language_support/tests/fixtures/server.rs");
    assert!(
        std::process::Command::new("rustc")
            .arg(fixture)
            .arg("-o")
            .arg(&mock)
            .status()?
            .success()
    );
    let wakeups = Arc::new(AtomicUsize::new(0));
    let observed = wakeups.clone();
    let service = LanguageService::with_server(
        dir.path().into(),
        Some(ServerCommand {
            program: mock,
            args: Vec::new(),
        }),
        move || {
            observed.fetch_add(1, Ordering::Relaxed);
        },
    );
    let uri = Url::from_file_path(dir.path().join("active.php")).map_err(|()| "URI")?;
    let mut terminal = TerminalManager::new(Arc::new(|| {}));
    let mut options = SessionOptions::new(dir.path().into());
    options.shell=Some(ShellCommand{program:"/bin/sh".into(),args:vec!["-c".into(),"i=0; while [ $i -lt 10000 ]; do printf 'stream %s\\r\\n' $i; i=$((i+1)); if [ $((i%100)) -eq 0 ]; then sleep 0.01; fi; done; sleep 1".into()]});
    let id = terminal.create(options)?;
    let git = nozhik_vcs::Vcs::start();
    assert!(git.send(1, 1, nozhik_vcs::Task::Open(dir.path().into())));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut revision = 0;
    let mut highlights = false;
    let mut diagnostics = false;
    let mut git_replies = 0;
    let mut terminal_ready = false;
    while Instant::now() < deadline {
        if revision == 0 || (diagnostics && revision < 20) {
            revision += 1;
            assert!(service.send(LanguageCommand::Sync {
                uri: uri.clone(),
                revision,
                text: ropey::Rope::from_str(&format!("<?php $value = {revision};"))
            }));
        }
        for event in service.drain() {
            match event {
                LanguageEvent::Highlights { revision: 20, .. } => highlights = true,
                LanguageEvent::Diagnostics { .. } => diagnostics = true,
                _ => {}
            }
        }
        for reply in git.drain() {
            assert!(reply.result.is_ok());
            git_replies += 1;
            if git_replies < 5 {
                assert!(git.send(1, git_replies + 1, nozhik_vcs::Task::Refresh));
            }
        }
        terminal_ready = terminal
            .sessions()
            .find(|s| s.id() == id)
            .and_then(nozhik_terminal::TerminalSession::snapshot)
            .is_some_and(|s| s.grid.is_some());
        if highlights && diagnostics && git_replies >= 5 && terminal_ready {
            break;
        }
        std::thread::sleep(Duration::from_millis(15));
    }
    assert!(
        highlights && diagnostics && git_replies >= 5 && terminal_ready,
        "highlights={highlights}, diagnostics={diagnostics}, git={git_replies}, terminal={terminal_ready}"
    );
    assert!(wakeups.load(Ordering::Relaxed) > 0);
    service.shutdown();
    terminal.shutdown();
    Ok(())
}
