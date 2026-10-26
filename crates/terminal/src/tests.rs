use crate::{
    SessionState, TerminalEvent, TerminalManager, Wakeup,
    grid::{Emulator, TerminalColor},
    pty::{PtyProcess, SessionOptions, ShellCommand, size},
};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};
type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn options(root: &std::path::Path) -> SessionOptions {
    let mut options = SessionOptions::new(root.into());
    #[cfg(unix)]
    {
        let script = r#"printf '\033[32mPTY_READY\033[0m\n'; while IFS= read -r line; do case "$line" in ping) printf 'PTY_REPLY\n';; size) stty size;; quit) exit 7;; *) printf '%s\n' "$line";; esac; done"#;
        options.shell = Some(ShellCommand {
            program: "/bin/sh".into(),
            args: vec!["-c".into(), script.into()],
        });
    }
    #[cfg(windows)]
    {
        options.shell = Some(ShellCommand {
            program: "cmd.exe".into(),
            args: vec!["/D".into(), "/Q".into()],
        });
    }
    options
}
fn wait_for(mut condition: impl FnMut() -> bool) -> TestResult {
    let until = Instant::now() + Duration::from_secs(8);
    while Instant::now() < until {
        if condition() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    Err("terminal condition timed out".into())
}
fn text(session: &crate::TerminalSession) -> String {
    session
        .snapshot()
        .and_then(|s| s.grid)
        .map_or_else(String::new, |g| {
            g.cells.iter().map(|c| c.text.as_str()).collect()
        })
}
fn command(unix: &[u8], windows: &[u8]) -> Vec<u8> {
    if cfg!(windows) {
        windows.to_vec()
    } else {
        unix.to_vec()
    }
}
fn ready(session: &crate::TerminalSession) -> TestResult {
    if cfg!(windows) {
        session.write(b"echo PTY_READY\r".to_vec())?;
    }
    wait_for(|| text(session).contains("PTY_READY"))
}

#[test]
fn ansi_colors_styles_cursor_clear_unicode_and_device_queries() {
    let mut emulator = Emulator::new(5, 40);
    emulator.process(b"\x1b[31;48;2;1;2;3mR\x1b[38;5;202;1;2;3;4;7mX");
    let grid = emulator.snapshot();
    assert_eq!(
        grid.cell(0, 0).map(|c| c.foreground),
        Some(TerminalColor::Indexed(1))
    );
    assert_eq!(
        grid.cell(0, 0).map(|c| c.background),
        Some(TerminalColor::Rgb(1, 2, 3))
    );
    let style = grid.cell(0, 1).map(|c| c.style);
    assert!(style.is_some_and(|s| !s.bold && s.dim && s.italic && s.underline && s.inverse));
    assert_eq!(
        grid.cell(0, 1).map(|c| c.foreground),
        Some(TerminalColor::Indexed(202))
    );
    emulator.process(b"\x1b[22;1mB");
    assert!(
        emulator
            .snapshot()
            .cell(0, 2)
            .is_some_and(|c| c.style.bold && !c.style.dim)
    );
    emulator.process(b"\x1b[0m\x1b[3;4H");
    assert_eq!(
        emulator.process(b"\x1b[6n\x1b[5n\x1b[c"),
        b"\x1b[3;4R\x1b[0n\x1b[?1;2c"
    );
    emulator.process(b"\x1b[2J\x1b[H");
    let utf8 = "Ж🦀e\u{301}".as_bytes();
    for byte in utf8 {
        emulator.process(&[*byte]);
    }
    let grid = emulator.snapshot();
    assert_eq!(grid.cell(0, 0).map(|c| c.character), Some('Ж'));
    assert!(grid.cell(0, 1).is_some_and(|c| c.wide));
    assert!(grid.cell(0, 2).is_some_and(|c| c.continuation));
    assert_eq!(grid.cell(0, 3).map(|c| c.text.as_str()), Some("e\u{301}"));
    emulator.process(b"\x1b[?25l\x1b[?1h\x1b[?2004h\x1b]2;My terminal\x07");
    let grid = emulator.snapshot();
    assert!(!grid.cursor_visible);
    assert!(grid.application_cursor && grid.bracketed_paste);
    assert_eq!(grid.title, "My terminal");
}
#[test]
fn alternate_buffer_scrollback_and_resize_are_independent() {
    let mut emulator = Emulator::new(3, 20);
    for n in 0..3010 {
        emulator.process(format!("line {n}\r\n").as_bytes());
    }
    emulator.scroll(i32::MAX);
    assert_eq!(emulator.snapshot().scrollback, 3000);
    emulator.live();
    let normal = emulator
        .snapshot()
        .cells
        .iter()
        .map(|c| c.text.as_str())
        .collect::<String>();
    emulator.process(b"\x1b[?1049h\x1b[H\x1b[2JALT");
    emulator.scroll(40);
    let alternate = emulator.snapshot();
    assert!(alternate.alternate);
    assert_eq!(alternate.scrollback, 0);
    assert_eq!(alternate.cell(0, 0).map(|c| c.character), Some('A'));
    emulator.process(b"\x1b[?1049l");
    assert_eq!(
        emulator
            .snapshot()
            .cells
            .iter()
            .map(|c| c.text.as_str())
            .collect::<String>(),
        normal
    );
    emulator.resize(1, 1);
    emulator.process(b"\x1b8X"); // restore cursor after shrinking must stay in bounds
    let grid = emulator.snapshot();
    assert_eq!(grid.cells.len(), 1);
    assert!(grid.cursor.0 < 1 && grid.cursor.1 < 1);
}
#[test]
fn real_pty_resize_reports_geometry_and_reaps_child() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut process = PtyProcess::spawn(&options(dir.path()))?;
    process
        .master
        .resize(size(37, 91))
        .map_err(crate::pty::error)?;
    let actual = process.master.get_size().map_err(crate::pty::error)?;
    assert_eq!((actual.rows, actual.cols), (37, 91));
    process.child.kill()?;
    let _status = process.child.wait()?;
    assert!(process.child.try_wait()?.is_some());
    Ok(())
}
#[test]
fn parallel_sessions_echo_resize_close_and_exit_release_processes() -> TestResult {
    let dir = tempfile::tempdir()?;
    let wake: Wakeup = Arc::new(|| {});
    let mut manager = TerminalManager::new(wake);
    let first = manager.create(options(dir.path()))?;
    let second = manager.create(options(dir.path()))?;
    ready(manager.session(first).ok_or("first missing")?)?;
    ready(manager.session(second).ok_or("second missing")?)?;
    assert_eq!(manager.sessions().count(), 2);
    let session = manager.session(first).ok_or("first missing")?;
    session.write(command(b"ping\r", b"echo PTY_REPLY\r"))?;
    wait_for(|| text(session).contains("PTY_REPLY"))?;
    session.resize(37, 91)?;
    wait_for(|| {
        session
            .snapshot()
            .and_then(|s| s.grid)
            .is_some_and(|g| (g.rows, g.cols) == (37, 91))
    })?;
    #[cfg(unix)]
    {
        session.write(b"size\r".to_vec())?;
        wait_for(|| text(session).contains("37 91"))?;
    }
    let closed = manager.remove(first).ok_or("remove failed")?;
    assert!(closed.wait_closed(Duration::from_secs(3)));
    assert_eq!(
        closed.snapshot().map(|s| s.state),
        Some(SessionState::Closed)
    );
    assert_eq!(manager.sessions().count(), 1);
    assert!(manager.session(first).is_none());
    let session = manager.session(second).ok_or("second missing")?;
    session.write(command(b"ping\r", b"echo PTY_REPLY\r"))?;
    wait_for(|| text(session).contains("PTY_REPLY"))?;
    session.write(command(b"quit\r", b"exit 7\r"))?;
    wait_for(|| {
        session
            .snapshot()
            .is_some_and(|s| s.state == SessionState::Exited(7))
    })?;
    assert!(session.wait_closed(Duration::from_secs(3)));
    assert!(
        manager
            .drain()
            .any(|event| matches!(event,TerminalEvent::Exited { id,code:7 } if id==second))
    );
    let _removed = manager.remove(second);
    assert_eq!(manager.sessions().count(), 0);
    Ok(())
}
#[test]
fn spawn_failure_is_observable_and_close_during_startup_is_safe() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut manager = TerminalManager::new(Arc::new(|| {}));
    let mut bad = options(dir.path());
    bad.shell = Some(ShellCommand {
        program: dir.path().join("missing-shell").into_os_string(),
        args: Vec::new(),
    });
    let failed = manager.create(bad)?;
    let session = manager.session(failed).ok_or("missing")?;
    wait_for(|| {
        session
            .snapshot()
            .is_some_and(|s| matches!(s.state, SessionState::Failed(_)))
    })?;
    assert!(session.wait_closed(Duration::from_secs(3)));
    assert!(
        manager
            .drain()
            .any(|e| matches!(e,TerminalEvent::Error { id,.. } if id==failed))
    );
    let pending = manager.create(options(dir.path()))?;
    let closed = manager.remove(pending).ok_or("missing")?;
    assert!(closed.wait_closed(Duration::from_secs(3)));
    assert_eq!(
        closed.snapshot().map(|s| s.state),
        Some(SessionState::Closed)
    );
    manager.shutdown();
    Ok(())
}

#[cfg(unix)]
#[test]
fn control_c_signals_foreground_and_close_unblocks_a_saturated_writer() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut manager = TerminalManager::new(Arc::new(|| {}));
    let mut opts = SessionOptions::new(dir.path().into());
    opts.shell = Some(ShellCommand { program:"/bin/sh".into(),args:vec!["-c".into(),r#"trap 'printf "SIGINT_OK\n"; exit 0' INT; printf 'WAITING\n'; while :; do sleep 1; done"#.into()] });
    let interrupt = manager.create(opts)?;
    let session = manager.session(interrupt).ok_or("missing")?;
    wait_for(|| text(session).contains("WAITING"))?;
    session.write(vec![3])?;
    wait_for(|| {
        session
            .snapshot()
            .is_some_and(|s| s.state == SessionState::Exited(0))
    })?;
    assert!(text(session).contains("SIGINT_OK"));
    let mut opts = SessionOptions::new(dir.path().into());
    opts.shell = Some(ShellCommand {
        program: "/bin/sh".into(),
        args: vec!["-c".into(), "printf 'BLOCKED\n'; sleep 30".into()],
    });
    let blocked = manager.create(opts)?;
    let session = manager.session(blocked).ok_or("missing")?;
    wait_for(|| text(session).contains("BLOCKED"))?;
    let mut busy = false;
    for _ in 0..128 {
        match session.write(vec![b'x'; 64 * 1024]) {
            Ok(()) => {}
            Err(crate::TerminalError::Busy) => {
                busy = true;
                break;
            }
            Err(error) => return Err(error.into()),
        }
    }
    assert!(busy, "bounded input must apply backpressure");
    let closed = manager.remove(blocked).ok_or("missing")?;
    assert!(closed.wait_closed(Duration::from_secs(3)));
    assert!(closed.wait_closed(Duration::ZERO));
    assert_eq!(
        closed.snapshot().map(|s| s.state),
        Some(SessionState::Closed)
    );
    manager.shutdown();
    Ok(())
}

#[cfg(unix)]
#[test]
#[ignore = "requires system /usr/bin/vi and a real PTY; run explicitly"]
fn system_vi_enters_alternate_screen_accepts_input_and_exits() -> TestResult {
    let dir = tempfile::tempdir()?;
    let mut manager = TerminalManager::new(Arc::new(|| {}));
    let mut opts = SessionOptions::new(dir.path().into());
    opts.shell = Some(ShellCommand {
        program: "/usr/bin/vi".into(),
        args: vec![
            "-u".into(),
            "NONE".into(),
            "-i".into(),
            "NONE".into(),
            "-n".into(),
        ],
    });
    let id = manager.create(opts)?;
    let session = manager.session(id).ok_or("missing")?;
    wait_for(|| {
        session
            .snapshot()
            .and_then(|s| s.grid)
            .is_some_and(|g| g.alternate)
    })?;
    session.write(b"iNozhik terminal".to_vec())?;
    wait_for(|| text(session).contains("Nozhik terminal"))?;
    session.write(vec![27])?;
    std::thread::sleep(Duration::from_millis(100));
    session.write(b":q!\r".to_vec())?;
    wait_for(|| {
        session
            .snapshot()
            .is_some_and(|s| s.state == SessionState::Exited(0))
    })?;
    assert!(session.wait_closed(Duration::from_secs(3)));
    manager.shutdown();
    Ok(())
}

#[test]
fn external_transport_uses_ansi_grid_resize_input_and_close() -> TestResult {
    use crate::external::{ExternalOutput, ExternalTransport};
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    struct Transport {
        output: mpsc::Receiver<ExternalOutput>,
        input: mpsc::Sender<Vec<u8>>,
        sizes: mpsc::Sender<(u16, u16)>,
        closed: Arc<AtomicBool>,
    }
    impl ExternalTransport for Transport {
        fn poll(&mut self) -> crate::Result<Option<ExternalOutput>> {
            Ok(self.output.try_recv().ok())
        }
        fn write(&mut self, bytes: Vec<u8>) -> crate::Result<()> {
            self.input
                .send(bytes)
                .map_err(|_| crate::TerminalError::Closed)
        }
        fn resize(&mut self, rows: u16, cols: u16) -> crate::Result<()> {
            self.sizes
                .send((rows, cols))
                .map_err(|_| crate::TerminalError::Closed)
        }
        fn close(&mut self) {
            self.closed.store(true, Ordering::Release);
        }
    }
    let (output, receiver) = mpsc::channel();
    let (input, writes) = mpsc::channel();
    let (sizes, resizes) = mpsc::channel();
    let closed = Arc::new(AtomicBool::new(false));
    let mut manager = TerminalManager::new(Arc::new(|| {}));
    let id = manager.create_external(
        SessionOptions::new(std::path::PathBuf::new()),
        Box::new(Transport {
            output: receiver,
            input,
            sizes,
            closed: closed.clone(),
        }),
    )?;
    let session = manager.session(id).ok_or("session missing")?;
    output.send(ExternalOutput::Data(
        b"\x1b[32mREMOTE_READY\x1b[0m".to_vec(),
    ))?;
    wait_for(|| text(session).contains("REMOTE_READY"))?;
    session.write(b"typed".to_vec())?;
    assert_eq!(writes.recv_timeout(Duration::from_secs(2))?, b"typed");
    session.resize(40, 100)?;
    assert_eq!(resizes.recv_timeout(Duration::from_secs(2))?, (40, 100));
    wait_for(|| {
        session
            .snapshot()
            .and_then(|s| s.grid)
            .is_some_and(|g| g.rows == 40 && g.cols == 100)
    })?;
    let session = manager.remove(id).ok_or("session missing")?;
    assert!(session.wait_closed(Duration::from_secs(2)));
    assert!(closed.load(Ordering::Acquire));
    Ok(())
}
