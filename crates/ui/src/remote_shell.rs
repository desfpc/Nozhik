//! Bounded asynchronous SSH channel adapter for the terminal's byte transport.
use nozhik_remote_ssh::{OPERATION_TIMEOUT, SshShellStream};
use nozhik_terminal::{
    TerminalError,
    external::{ExternalOutput, ExternalTransport},
};
use tokio::sync::{mpsc, oneshot};

enum Input {
    Data(Vec<u8>),
    Resize(u16, u16),
}
pub struct RemoteShell {
    input: mpsc::Sender<Input>,
    output: mpsc::Receiver<nozhik_terminal::Result<ExternalOutput>>,
    stop: Option<oneshot::Sender<()>>,
}
impl RemoteShell {
    pub fn start(mut shell: SshShellStream) -> Self {
        let (input, mut commands) = mpsc::channel(64);
        let (sender, output) = mpsc::channel(64);
        let (stop, stopped) = oneshot::channel();
        // Detach the task; the independent stop signal cancels I/O then closes the channel.
        let _task = tokio::spawn(async move {
            let run = async {
                let mut code = None;
                let result = async {
                loop {
                    tokio::select! {
                        command = commands.recv() => {
                            let operation = async {
                                match command {
                                    Some(Input::Data(bytes)) => shell.channel.data(&bytes[..]).await,
                                    Some(Input::Resize(rows, cols)) => shell.channel.window_change(u32::from(cols), u32::from(rows), 0, 0).await,
                                    None => return Ok(false),
                                }?;
                                Ok::<_, nozhik_remote_ssh::Error>(true)
                            };
                            if !tokio::time::timeout(OPERATION_TIMEOUT, operation).await?? { break; }
                        }
                        message = shell.wait() => {
                            match message {
                                Some(russh::ChannelMsg::Data { data } | russh::ChannelMsg::ExtendedData { data, .. }) => {
                                    if sender.send(Ok(ExternalOutput::Data(data.to_vec()))).await.is_err() { break; }
                                }
                                Some(russh::ChannelMsg::ExitStatus { exit_status }) => code = Some(exit_status),
                                Some(russh::ChannelMsg::ExitSignal { .. }) => code = Some(128),
                                Some(russh::ChannelMsg::Close) | None => {
                                    if let Some(code) = code {
                                        let _delivered = sender.send(Ok(ExternalOutput::Exited(code))).await;
                                    } else { return Err(nozhik_remote_ssh::Error::Invalid("SSH shell closed without exit status (connection lost)".into())); }
                                    break;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                Ok::<_, nozhik_remote_ssh::Error>(())
            }.await;
                if let Err(error) = result {
                    let _delivered = sender
                        .send(Err(TerminalError::Pty(error.to_string())))
                        .await;
                }
            };
            tokio::select! { () = run => {}, _ = stopped => {} }
            // Closing a tab must explicitly notify the peer even with full queues.
            let _closed =
                tokio::time::timeout(std::time::Duration::from_secs(2), shell.channel.close())
                    .await;
        });
        Self {
            input,
            output,
            stop: Some(stop),
        }
    }
    fn send(&self, input: Input) -> nozhik_terminal::Result<()> {
        self.input.try_send(input).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => TerminalError::Busy,
            mpsc::error::TrySendError::Closed(_) => TerminalError::Closed,
        })
    }
}
impl ExternalTransport for RemoteShell {
    fn poll(&mut self) -> nozhik_terminal::Result<Option<ExternalOutput>> {
        match self.output.try_recv() {
            Ok(result) => result.map(Some),
            Err(mpsc::error::TryRecvError::Empty) => Ok(None),
            Err(mpsc::error::TryRecvError::Disconnected) => Err(TerminalError::Closed),
        }
    }
    fn write(&mut self, bytes: Vec<u8>) -> nozhik_terminal::Result<()> {
        self.send(Input::Data(bytes))
    }
    fn resize(&mut self, rows: u16, cols: u16) -> nozhik_terminal::Result<()> {
        self.send(Input::Resize(rows, cols))
    }
    fn close(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _sent = stop.send(());
        }
    }
}
impl Drop for RemoteShell {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _sent = stop.send(());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh::{Channel, ChannelId, server};
    use std::{
        future::{Future, ready},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };
    struct Server(Arc<AtomicBool>);
    impl server::Handler for Server {
        type Error = russh::Error;
        fn auth_password(
            &mut self,
            _user: &str,
            _password: &str,
        ) -> impl Future<Output = Result<server::Auth, Self::Error>> + Send {
            ready(Ok(server::Auth::Accept))
        }
        async fn channel_open_session(
            &mut self,
            _channel: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> Result<(), Self::Error> {
            reply.accept().await;
            Ok(())
        }
        fn pty_request(
            &mut self,
            channel: ChannelId,
            _term: &str,
            _cols: u32,
            _rows: u32,
            _x: u32,
            _y: u32,
            _modes: &[(russh::Pty, u32)],
            session: &mut server::Session,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send {
            ready(session.channel_success(channel))
        }
        fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut server::Session,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send {
            let result = (|| {
                session.channel_success(channel)?;
                for _ in 0..80 {
                    session.data(channel, vec![b'x'; 1024])?;
                }
                Ok(())
            })();
            ready(result)
        }
        fn channel_close(
            &mut self,
            _channel: ChannelId,
            _session: &mut server::Session,
        ) -> impl Future<Output = Result<(), Self::Error>> + Send {
            self.0.store(true, Ordering::Release);
            ready(Ok(()))
        }
    }
    #[tokio::test]
    async fn closing_shell_notifies_peer_even_with_saturated_output()
    -> Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let key =
                russh::keys::PrivateKey::random(&mut rand::rng(), russh::keys::Algorithm::Ed25519)?;
            let pin = key
                .public_key()
                .fingerprint(russh::keys::HashAlg::Sha256)
                .to_string();
            let config = Arc::new(server::Config {
                keys: vec![key],
                ..Default::default()
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
            let port = listener.local_addr()?.port();
            let closed = Arc::new(AtomicBool::new(false));
            let observed = closed.clone();
            let task = tokio::spawn(async move {
                let (socket, _) = listener.accept().await?;
                server::run_stream(config, socket, Server(observed))
                    .await?
                    .await
            });
            let profile = nozhik_remote_ssh::SshProfile {
                name: "test".into(),
                host: "127.0.0.1".into(),
                port,
                username: "user".into(),
                auth: nozhik_remote_ssh::AuthMethod::Password,
                host_key_sha256: pin,
                save_password: false,
                forwards: Vec::new(),
            };
            let ssh = nozhik_remote_ssh::SshSession::connect(&profile, Some("secret")).await?;
            let mut shell = RemoteShell::start(ssh.open_shell(24, 80).await?);
            while shell.output.len() < 64 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            shell.close();
            while !closed.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            task.abort();
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        Ok(())
    }
}
