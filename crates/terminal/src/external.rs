//! Nonblocking transport boundary for remote shells; no SSH dependency in terminal.
use crate::Result;
/// One remote output update.
pub enum ExternalOutput {
    /// Raw terminal bytes, including ANSI escapes.
    Data(Vec<u8>),
    /// Remote shell's exit status.
    Exited(u32),
}
/// Transport methods run on the terminal worker and must not wait for network I/O.
/// Drop must cancel the underlying transport even if startup or a write failed.
pub trait ExternalTransport: Send {
    /// Poll available output; None means temporarily empty.
    /// # Errors
    /// Returns connection failure, never panics on EOF.
    fn poll(&mut self) -> Result<Option<ExternalOutput>>;
    /// Queue exact input bytes.
    /// # Errors
    /// Returns backpressure or disconnected transport errors.
    fn write(&mut self, bytes: Vec<u8>) -> Result<()>;
    /// Queue a remote window-change request (rows, columns).
    /// # Errors
    /// Returns queue/transport errors.
    fn resize(&mut self, rows: u16, cols: u16) -> Result<()>;
    /// Cancel channel independently of saturated input queues.
    fn close(&mut self);
}
