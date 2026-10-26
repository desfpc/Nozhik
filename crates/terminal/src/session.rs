//! Nonblocking UI handles; control, read and write operations run on workers.
use crate::{
    Result, TerminalError, TerminalEvent, TerminalSessionId, Wakeup,
    grid::{Emulator, TerminalGrid},
    pty::{self, SessionOptions},
};
use parking_lot::Mutex;
use std::{
    io::{Read, Write},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// Authoritative session lifecycle.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionState {
    /// Worker has not completed PTY creation.
    Starting,
    /// Live shell with optional native process ID.
    Running(Option<u32>),
    /// Child exited naturally and was reaped.
    Exited(u32),
    /// Creation or process polling failed.
    Failed(String),
    /// Explicitly closed, process terminated and reaped.
    Closed,
}
/// Immutable snapshot; cloning its grid is an Arc operation.
#[derive(Clone, Debug)]
pub struct SessionSnapshot {
    /// Latest parsed visible grid, absent while spawning.
    pub grid: Option<Arc<TerminalGrid>>,
    /// Current lifecycle state.
    pub state: SessionState,
}
enum Control {
    Resize(u16, u16),
    Scroll(i32),
}
struct Shared {
    emulator: Mutex<Emulator>,
    snapshot: Mutex<SessionSnapshot>,
    stop: AtomicBool,
    live: AtomicBool,
    reader_done: AtomicBool,
    completed: AtomicBool,
}
impl Shared {
    fn publish(&self, emulator: &Emulator) {
        self.snapshot.lock().grid = Some(Arc::new(emulator.snapshot()));
    }
    fn state(&self, state: SessionState) {
        self.snapshot.lock().state = state;
    }
}
/// Handle for one asynchronous PTY session; Drop signals close without waiting.
pub struct TerminalSession {
    id: TerminalSessionId,
    options: SessionOptions,
    shared: Arc<Shared>,
    input: mpsc::SyncSender<Vec<u8>>,
    control: mpsc::SyncSender<Control>,
    done: mpsc::Receiver<()>,
}
impl TerminalSession {
    /// Schedules shell startup. No PTY, PATH or filesystem I/O occurs here.
    #[must_use]
    pub fn start(
        id: TerminalSessionId,
        options: SessionOptions,
        wake: Wakeup,
        events: mpsc::Sender<TerminalEvent>,
    ) -> Self {
        Self::start_backend(id, options, wake, events, None)
    }
    /// Start a terminal backed by a nonblocking external byte transport.
    #[must_use]
    pub fn start_external(
        id: TerminalSessionId,
        options: SessionOptions,
        wake: Wakeup,
        events: mpsc::Sender<TerminalEvent>,
        transport: Box<dyn crate::external::ExternalTransport>,
    ) -> Self {
        Self::start_backend(id, options, wake, events, Some(transport))
    }
    fn start_backend(
        id: TerminalSessionId,
        options: SessionOptions,
        wake: Wakeup,
        events: mpsc::Sender<TerminalEvent>,
        transport: Option<Box<dyn crate::external::ExternalTransport>>,
    ) -> Self {
        let shared = Arc::new(Shared {
            emulator: Mutex::new(Emulator::new(options.rows, options.cols)),
            snapshot: Mutex::new(SessionSnapshot {
                grid: None,
                state: SessionState::Starting,
            }),
            stop: AtomicBool::new(false),
            live: AtomicBool::new(false),
            reader_done: AtomicBool::new(false),
            completed: AtomicBool::new(false),
        });
        let (input, writes) = mpsc::sync_channel(64);
        let (control, commands) = mpsc::sync_channel(128);
        let (finished, done) = mpsc::channel();
        let state = shared.clone();
        let worker_options = options.clone();
        std::thread::spawn(move || {
            let result = if let Some(transport) = transport {
                external_worker(id, transport, &state, &wake, &events, &writes, &commands)
            } else {
                worker(
                    id,
                    &worker_options,
                    &state,
                    &wake,
                    &events,
                    writes,
                    &commands,
                )
            };
            if let Err(error) = result {
                state.state(SessionState::Failed(error.to_string()));
                report(
                    &events,
                    &wake,
                    TerminalEvent::Error {
                        id,
                        message: error.to_string(),
                    },
                );
            }
            state.stop.store(true, Ordering::Release);
            state.completed.store(true, Ordering::Release);
            let _sent = finished.send(());
        });
        Self {
            id,
            options,
            shared,
            input,
            control,
            done,
        }
    }
    /// Stable session identity.
    #[must_use]
    pub const fn id(&self) -> TerminalSessionId {
        self.id
    }
    /// Initial invocation and cwd; sessions retain this cwd when the project changes.
    #[must_use]
    pub const fn options(&self) -> &SessionOptions {
        &self.options
    }
    /// Clones the latest snapshot without waiting for a lock; callers retain their
    /// previous snapshot if a concurrent update is in progress.
    #[must_use]
    pub fn snapshot(&self) -> Option<SessionSnapshot> {
        self.shared.snapshot.try_lock().map(|s| s.clone())
    }
    /// Enqueues exact input bytes (up to 64 KiB). Never writes a pipe on the caller.
    /// # Errors
    /// Returns backpressure, oversize or closed-session errors. On error the whole
    /// batch was rejected; partial enqueue never occurs.
    pub fn write(&self, bytes: Vec<u8>) -> Result<()> {
        if self.shared.stop.load(Ordering::Acquire) {
            return Err(TerminalError::Closed);
        }
        if bytes.len() > 64 * 1024 {
            return Err(TerminalError::Invalid(
                "terminal input batch exceeds 64 KiB".into(),
            ));
        }
        self.input.try_send(bytes).map_err(queue_error)?;
        self.shared.live.store(true, Ordering::Release);
        Ok(())
    }
    /// Schedules PTY and emulator resize. Invalid values leave the session unchanged.
    /// # Errors
    /// Returns geometry, backpressure or disconnected-worker errors.
    pub fn resize(&self, rows: u16, cols: u16) -> Result<()> {
        if !(1..=200).contains(&rows) || !(1..=500).contains(&cols) {
            return Err(TerminalError::Invalid("invalid terminal geometry".into()));
        }
        self.control
            .try_send(Control::Resize(rows, cols))
            .map_err(queue_error)
    }
    /// Scrolls history in rows. Full-screen alternate buffers ignore history scroll.
    /// # Errors
    /// Returns backpressure or disconnected-worker errors.
    pub fn scroll(&self, rows: i32) -> Result<()> {
        self.control
            .try_send(Control::Scroll(rows))
            .map_err(queue_error)
    }
    /// Requests termination independently of saturated I/O command queues.
    pub fn close(&self) {
        self.shared.stop.store(true, Ordering::Release);
    }
    /// Waits for control cleanup; for tests/application exit, never a normal frame.
    #[must_use]
    pub fn wait_closed(&self, timeout: Duration) -> bool {
        self.shared.completed.load(Ordering::Acquire) || self.done.recv_timeout(timeout).is_ok()
    }
}
impl Drop for TerminalSession {
    fn drop(&mut self) {
        self.close();
    }
}
#[allow(
    clippy::needless_pass_by_value,
    reason = "map_err consumes the rejected input batch"
)]
fn queue_error<T>(error: mpsc::TrySendError<T>) -> TerminalError {
    match error {
        mpsc::TrySendError::Full(_) => TerminalError::Busy,
        mpsc::TrySendError::Disconnected(_) => TerminalError::Closed,
    }
}
fn report(events: &mpsc::Sender<TerminalEvent>, wake: &Wakeup, event: TerminalEvent) {
    let _sent = events.send(event);
    wake();
}
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "Worker receives the session's owned queues and platform-independent state"
)]
fn worker(
    id: TerminalSessionId,
    options: &SessionOptions,
    shared: &Arc<Shared>,
    wake: &Wakeup,
    events: &mpsc::Sender<TerminalEvent>,
    writes: mpsc::Receiver<Vec<u8>>,
    commands: &mpsc::Receiver<Control>,
) -> Result<()> {
    if shared.stop.load(Ordering::Acquire) {
        shared.state(SessionState::Closed);
        return Ok(());
    }
    let pty::PtyProcess {
        master,
        mut child,
        reader,
        writer,
    } = pty::PtyProcess::spawn(options)?;
    let (replies, reply_input) = mpsc::sync_channel(64);
    let output_shared = shared.clone();
    let output_wake = wake.clone();
    let output_events = events.clone();
    let read_thread = std::thread::spawn(move || {
        read_output(
            id,
            reader,
            &output_shared,
            &output_wake,
            &output_events,
            &replies,
        );
    });
    let input_shared = shared.clone();
    let input_wake = wake.clone();
    let input_events = events.clone();
    let write_thread = std::thread::spawn(move || {
        write_input(
            id,
            writer,
            &writes,
            &reply_input,
            &input_shared,
            &input_wake,
            &input_events,
        );
    });
    let pid = child.process_id();
    shared.state(SessionState::Running(pid));
    report(events, wake, TerminalEvent::Started { id, pid });
    let result = loop {
        if shared.stop.load(Ordering::Acquire) {
            terminate(&*master, &mut *child);
            shared.state(SessionState::Closed);
            break Ok(());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                // Give the reader a bounded chance to publish the tail before Exited.
                let until = Instant::now() + Duration::from_millis(100);
                while !shared.reader_done.load(Ordering::Acquire) && Instant::now() < until {
                    std::thread::sleep(Duration::from_millis(2));
                }
                shared.state(SessionState::Exited(status.exit_code()));
                report(
                    events,
                    wake,
                    TerminalEvent::Exited {
                        id,
                        code: status.exit_code(),
                    },
                );
                break Ok(());
            }
            Ok(None) => {}
            Err(error) => {
                terminate(&*master, &mut *child);
                break Err(error.into());
            }
        }
        if shared.live.swap(false, Ordering::AcqRel) {
            let mut emulator = shared.emulator.lock();
            emulator.live();
            shared.publish(&emulator);
            drop(emulator);
            wake();
        }
        match commands.recv_timeout(Duration::from_millis(20)) {
            Ok(Control::Resize(rows, cols)) => {
                if let Err(error) = master.resize(pty::size(rows, cols)) {
                    report(
                        events,
                        wake,
                        TerminalEvent::Error {
                            id,
                            message: format!("resize: {error}"),
                        },
                    );
                } else {
                    let mut emulator = shared.emulator.lock();
                    emulator.resize(rows, cols);
                    shared.publish(&emulator);
                    drop(emulator);
                    wake();
                }
            }
            Ok(Control::Scroll(delta)) => {
                let mut emulator = shared.emulator.lock();
                emulator.scroll(delta);
                shared.publish(&emulator);
                drop(emulator);
                wake();
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                shared.stop.store(true, Ordering::Release);
            }
        }
    };
    shared.stop.store(true, Ordering::Release);
    drop(master);
    let until = Instant::now() + Duration::from_secs(1);
    while (!read_thread.is_finished() || !write_thread.is_finished()) && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    if read_thread.is_finished() {
        let _joined = read_thread.join();
    }
    if write_thread.is_finished() {
        let _joined = write_thread.join();
    }
    wake();
    result
}
fn terminate(
    #[cfg_attr(
        not(unix),
        allow(
            unused_variables,
            reason = "Only Unix exposes a foreground process group on the master"
        )
    )]
    master: &dyn portable_pty::MasterPty,
    child: &mut dyn portable_pty::Child,
) {
    #[cfg(unix)]
    let foreground = master.process_group_leader().filter(|pid| *pid > 1);
    #[cfg(unix)]
    if let Some(group) = foreground {
        let _signaled = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(group),
            nix::sys::signal::Signal::SIGHUP,
        );
    }
    let _killed = child.kill();
    #[cfg(unix)]
    if let Some(group) = foreground {
        let _signaled = nix::sys::signal::killpg(
            nix::unistd::Pid::from_raw(group),
            nix::sys::signal::Signal::SIGKILL,
        );
    }
    let _reaped = child.wait();
}
fn read_output(
    id: TerminalSessionId,
    mut reader: Box<dyn Read + Send>,
    shared: &Shared,
    wake: &Wakeup,
    events: &mpsc::Sender<TerminalEvent>,
    replies: &mpsc::SyncSender<Vec<u8>>,
) {
    let mut buffer = [0; 8192];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(count) => {
                let mut emulator = shared.emulator.lock();
                let response = emulator.process(&buffer[..count]);
                shared.publish(&emulator);
                drop(emulator);
                if !response.is_empty() && replies.try_send(response).is_err() {
                    report(
                        events,
                        wake,
                        TerminalEvent::Error {
                            id,
                            message: "terminal reply queue full/closed".into(),
                        },
                    );
                }
                wake();
            }
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            // Unix PTYs commonly report EIO at the slave's EOF.
            Err(error)
                if shared.stop.load(Ordering::Acquire)
                    || (cfg!(unix) && error.raw_os_error() == Some(5)) =>
            {
                break;
            }
            Err(error) => {
                report(
                    events,
                    wake,
                    TerminalEvent::Error {
                        id,
                        message: format!("read: {error}"),
                    },
                );
                break;
            }
        }
    }
    shared.reader_done.store(true, Ordering::Release);
}
fn write_input(
    id: TerminalSessionId,
    mut writer: Box<dyn Write + Send>,
    input: &mpsc::Receiver<Vec<u8>>,
    replies: &mpsc::Receiver<Vec<u8>>,
    shared: &Shared,
    wake: &Wakeup,
    events: &mpsc::Sender<TerminalEvent>,
) {
    while !shared.stop.load(Ordering::Acquire) {
        let batch = replies
            .try_recv()
            .ok()
            .or_else(|| input.recv_timeout(Duration::from_millis(10)).ok());
        if let Some(bytes) = batch
            && let Err(error) = writer.write_all(&bytes).and_then(|()| writer.flush())
        {
            if !shared.stop.load(Ordering::Acquire) {
                report(
                    events,
                    wake,
                    TerminalEvent::Error {
                        id,
                        message: format!("write: {error}"),
                    },
                );
            }
            break;
        }
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "External backend shares existing lifecycle and queue contracts"
)]
fn external_worker(
    id: TerminalSessionId,
    mut transport: Box<dyn crate::external::ExternalTransport>,
    shared: &Shared,
    wake: &Wakeup,
    events: &mpsc::Sender<TerminalEvent>,
    writes: &mpsc::Receiver<Vec<u8>>,
    commands: &mpsc::Receiver<Control>,
) -> Result<()> {
    use crate::external::ExternalOutput;
    shared.state(SessionState::Running(None));
    report(events, wake, TerminalEvent::Started { id, pid: None });
    let result = (|| {
        loop {
            if shared.stop.load(Ordering::Acquire) {
                shared.state(SessionState::Closed);
                return Ok(());
            }
            for bytes in writes.try_iter().take(64) {
                transport.write(bytes)?;
            }
            for command in commands.try_iter().take(128) {
                match command {
                    Control::Resize(rows, cols) => {
                        transport.resize(rows, cols)?;
                        let mut emulator = shared.emulator.lock();
                        emulator.resize(rows, cols);
                        shared.publish(&emulator);
                        drop(emulator);
                    }
                    Control::Scroll(delta) => {
                        let mut emulator = shared.emulator.lock();
                        emulator.scroll(delta);
                        shared.publish(&emulator);
                        drop(emulator);
                    }
                }
            }
            if shared.live.swap(false, Ordering::AcqRel) {
                let mut emulator = shared.emulator.lock();
                emulator.live();
                shared.publish(&emulator);
                drop(emulator);
            }
            for _ in 0..128 {
                match transport.poll()? {
                    Some(ExternalOutput::Data(bytes)) => {
                        let response = {
                            let mut emulator = shared.emulator.lock();
                            let response = emulator.process(&bytes);
                            shared.publish(&emulator);
                            drop(emulator);
                            response
                        };
                        if !response.is_empty() {
                            transport.write(response)?;
                        }
                    }
                    Some(ExternalOutput::Exited(code)) => {
                        shared.state(SessionState::Exited(code));
                        report(events, wake, TerminalEvent::Exited { id, code });
                        return Ok(());
                    }
                    None => break,
                }
            }
            wake();
            std::thread::sleep(Duration::from_millis(16));
        }
    })();
    transport.close();
    result
}
