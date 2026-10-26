//! Loopback-only local forwarding with cancellation of all accepted connections.
use crate::{Error, OPERATION_TIMEOUT, Result, SshSession};
use std::net::SocketAddr;
use tokio::{
    net::TcpListener,
    task::{JoinHandle, JoinSet},
};

/// Owned tunnel; dropping or stopping it closes its listener and active streams.
pub struct TunnelHandle {
    address: SocketAddr,
    task: JoinHandle<Result<()>>,
}
impl TunnelHandle {
    /// Actual loopback listener address, including allocated port for `local_port=0`.
    #[must_use]
    pub const fn local_addr(&self) -> SocketAddr {
        self.address
    }
    /// Whether the accept loop is still running.
    #[must_use]
    pub fn is_running(&self) -> bool {
        !self.task.is_finished()
    }
    /// Cancel listener and active forwarded streams.
    pub fn stop(&self) {
        self.task.abort();
    }
}
impl Drop for TunnelHandle {
    fn drop(&mut self) {
        self.stop();
    }
}
impl SshSession {
    /// Start forwarding loopback connections through direct-tcpip channels.
    /// # Errors
    /// Returns invalid target or local bind failure.
    pub async fn start_tunnel(
        &self,
        local_port: u16,
        remote_host: &str,
        remote_port: u16,
    ) -> Result<TunnelHandle> {
        if remote_host.trim().is_empty() || remote_port == 0 {
            return Err(Error::Invalid("invalid tunnel target".into()));
        }
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, local_port)).await?;
        let address = listener.local_addr()?;
        let ssh = self.clone();
        let remote_host = remote_host.to_owned();
        let task = tokio::spawn(async move {
            let mut streams = JoinSet::new();
            loop {
                tokio::select! {
                    connection = listener.accept(), if streams.len() < 64 => {
                        let (mut socket, origin) = connection?;
                        let ssh = ssh.clone(); let host = remote_host.clone();
                        streams.spawn(async move {
                            let channel = tokio::time::timeout(OPERATION_TIMEOUT, async {
                                ssh.handle.lock().await.channel_open_direct_tcpip(host, u32::from(remote_port), origin.ip().to_string(), u32::from(origin.port())).await
                            }).await??;
                            let mut channel = channel.into_stream();
                            tokio::io::copy_bidirectional(&mut socket, &mut channel).await?;
                            Ok::<_, Error>(())
                        });
                    }
                    Some(result) = streams.join_next(), if !streams.is_empty() => {
                        match result {
                            Ok(Ok(())) => {},
                            Ok(Err(e)) => eprintln!("SSH tunnel stream: {e}"),
                            Err(e) => eprintln!("SSH tunnel task: {e}"),
                        }
                    }
                    () = tokio::time::sleep(std::time::Duration::from_secs(1)) => {
                        if !ssh.is_connected().await { return Err(Error::Invalid("SSH tunnel disconnected".into())); }
                    }
                }
            }
        });
        Ok(TunnelHandle { address, task })
    }
}
