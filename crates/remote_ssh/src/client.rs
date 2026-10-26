//! Authenticated SSH connections with pinned host keys and bounded operations.
use crate::{AuthMethod, Error, OPERATION_TIMEOUT, Result, SshProfile};
use russh::{
    client,
    keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate, ssh_key::HashAlg},
};
use std::{collections::VecDeque, sync::Arc};
use tokio::sync::Mutex;

pub(crate) struct HostKey {
    expected: String,
    observed: tokio::sync::watch::Sender<Option<String>>,
}
impl client::Handler for HostKey {
    type Error = russh::Error;
    fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> impl Future<Output = std::result::Result<bool, Self::Error>> {
        let accepted = match key {
            PublicKeyOrCertificate::PublicKey { key, .. } => {
                let fingerprint = key.fingerprint(HashAlg::Sha256).to_string();
                let accepted = fingerprint == self.expected;
                self.observed.send_replace(Some(fingerprint));
                accepted
            }
            // Host certificates need CA policy; do not silently trust their embedded key.
            PublicKeyOrCertificate::Certificate(_) => false,
        };
        std::future::ready(Ok(accepted))
    }
}
/// Shared authenticated SSH transport; channels have independent lifetimes.
#[derive(Clone)]
pub struct SshSession {
    pub(crate) handle: Arc<Mutex<client::Handle<HostKey>>>,
}
impl SshSession {
    /// Connect and authenticate with a secret supplied only in memory.
    /// # Errors
    /// Rejects unknown/changed host keys, bad credentials and timed out transports.
    pub async fn connect(profile: &SshProfile, secret: Option<&str>) -> Result<Self> {
        profile.validate()?;
        tokio::time::timeout(OPERATION_TIMEOUT, Self::connect_inner(profile, secret)).await?
    }
    /// Verify the server identity without sending passwords or trying authentication.
    /// # Errors
    /// Unknown keys return `Error::UnknownHostKey` for an explicit trust decision;
    /// changed keys, transport failures and timeouts remain errors.
    pub async fn verify_host_key(profile: &SshProfile) -> Result<()> {
        profile.validate()?;
        tokio::time::timeout(OPERATION_TIMEOUT, async {
            let handle = transport(profile).await?;
            handle
                .disconnect(russh::Disconnect::ByApplication, "Host key verified", "en")
                .await?;
            Ok::<_, Error>(())
        })
        .await?
    }
    async fn connect_inner(profile: &SshProfile, secret: Option<&str>) -> Result<Self> {
        let mut handle = transport(profile).await?;
        let success = match &profile.auth {
            AuthMethod::Password => handle
                .authenticate_password(
                    &profile.username,
                    secret.ok_or_else(|| Error::Invalid("password required".into()))?,
                )
                .await?
                .success(),
            AuthMethod::PrivateKeyFile { path } => {
                let path = if let Ok(tail) = path.strip_prefix("~") {
                    directories::BaseDirs::new()
                        .ok_or_else(|| Error::Invalid("home directory unavailable".into()))?
                        .home_dir()
                        .join(tail)
                } else {
                    path.clone()
                };
                let phrase = secret.map(|s| crate::credentials::Secret::new(s.to_owned()));
                let key = tokio::task::spawn_blocking(move || {
                    russh::keys::load_secret_key(
                        path,
                        phrase.as_ref().map(crate::credentials::Secret::expose),
                    )
                })
                .await
                .map_err(|e| Error::Invalid(e.to_string()))??;
                let hash = handle.best_supported_rsa_hash().await?.flatten();
                handle
                    .authenticate_publickey(
                        &profile.username,
                        PrivateKeyWithHashAlg::new(Arc::new(key), hash),
                    )
                    .await?
                    .success()
            }
            AuthMethod::Agent => {
                #[cfg(unix)]
                let mut agent = russh::keys::agent::client::AgentClient::connect_env().await?;
                #[cfg(windows)]
                let mut agent = russh::keys::agent::client::AgentClient::connect_named_pipe(
                    r"\\.\pipe\openssh-ssh-agent",
                )
                .await?;
                let mut success = false;
                let hash = handle.best_supported_rsa_hash().await?.flatten();
                for identity in agent.request_identities().await? {
                    if let russh::keys::agent::AgentIdentity::PublicKey { key, .. } = identity
                        && handle
                            .authenticate_publickey_with(&profile.username, key, hash, &mut agent)
                            .await
                            .map_err(|e| Error::Invalid(e.to_string()))?
                            .success()
                    {
                        success = true;
                        break;
                    }
                }
                success
            }
        };
        if !success {
            return Err(Error::AuthenticationRejected);
        }
        Ok(Self {
            handle: Arc::new(Mutex::new(handle)),
        })
    }
    /// Whether the underlying SSH task still accepts channels.
    pub async fn is_connected(&self) -> bool {
        !self.handle.lock().await.is_closed()
    }
    /// Disconnect all channels gracefully.
    /// # Errors
    /// Returns transport or deadline failures.
    pub async fn disconnect(&self) -> Result<()> {
        tokio::time::timeout(OPERATION_TIMEOUT, async {
            self.handle
                .lock()
                .await
                .disconnect(russh::Disconnect::ByApplication, "Nozhik disconnect", "en")
                .await
        })
        .await??;
        Ok(())
    }
    /// Request an interactive xterm PTY and shell.
    /// # Errors
    /// Returns invalid dimensions, SSH channel rejection or deadline failures.
    pub async fn open_shell(&self, rows: u16, cols: u16) -> Result<SshShellStream> {
        if !(1..=200).contains(&rows) || !(1..=500).contains(&cols) {
            return Err(Error::Invalid("invalid terminal size".into()));
        }
        tokio::time::timeout(OPERATION_TIMEOUT, async {
            let mut channel = self.handle.lock().await.channel_open_session().await?;
            let mut pending = VecDeque::new();
            channel
                .request_pty(
                    true,
                    "xterm-256color",
                    u32::from(cols),
                    u32::from(rows),
                    0,
                    0,
                    &[],
                )
                .await?;
            accepted(&mut channel, "PTY", &mut pending).await?;
            channel.request_shell(true).await?;
            accepted(&mut channel, "shell", &mut pending).await?;
            Ok(SshShellStream { channel, pending })
        })
        .await?
    }
}
async fn transport(profile: &SshProfile) -> Result<client::Handle<HostKey>> {
    let config = client::Config {
        keepalive_interval: Some(std::time::Duration::from_secs(20)),
        keepalive_max: 3,
        ..Default::default()
    };
    let (observed, received) = tokio::sync::watch::channel(None);
    match client::connect(
        Arc::new(config),
        (profile.host.as_str(), profile.port),
        HostKey {
            expected: profile.host_key_sha256.clone(),
            observed,
        },
    )
    .await
    {
        Ok(handle) => Ok(handle),
        Err(error) => {
            let fingerprint = received.borrow().clone();
            if let Some(fingerprint) = fingerprint {
                if profile.host_key_sha256.is_empty() {
                    return Err(Error::UnknownHostKey(fingerprint));
                }
                if fingerprint != profile.host_key_sha256 {
                    return Err(Error::HostKeyChanged {
                        expected: profile.host_key_sha256.clone(),
                        received: fingerprint,
                    });
                }
            }
            Err(error.into())
        }
    }
}
async fn accepted(
    channel: &mut russh::Channel<client::Msg>,
    request: &str,
    pending: &mut VecDeque<russh::ChannelMsg>,
) -> Result<()> {
    let mut buffered = 0;
    loop {
        match channel.wait().await {
            Some(russh::ChannelMsg::Success) => return Ok(()),
            Some(russh::ChannelMsg::Failure) => {
                return Err(Error::Invalid(format!(
                    "server rejected {request} request; check server/account permissions"
                )));
            }
            Some(russh::ChannelMsg::WindowAdjusted { .. } | russh::ChannelMsg::XonXoff { .. }) => {}
            Some(
                message @ (russh::ChannelMsg::Data { .. } | russh::ChannelMsg::ExtendedData { .. }),
            ) => {
                if let russh::ChannelMsg::Data { data }
                | russh::ChannelMsg::ExtendedData { data, .. } = &message
                {
                    buffered += data.len();
                }
                if buffered > 64 * 1024 || pending.len() >= 128 {
                    return Err(Error::Invalid(format!(
                        "too much output before {request} acknowledgement"
                    )));
                }
                pending.push_back(message);
            }
            Some(russh::ChannelMsg::ExitStatus { exit_status }) => {
                return Err(Error::Invalid(format!(
                    "remote process exited with status {exit_status} before {request} acknowledgement"
                )));
            }
            Some(
                russh::ChannelMsg::Eof
                | russh::ChannelMsg::Close
                | russh::ChannelMsg::ExitSignal { .. },
            )
            | None => {
                return Err(Error::Invalid(format!(
                    "SSH channel closed while waiting for {request} acknowledgement"
                )));
            }
            _ => {
                return Err(Error::Invalid(format!(
                    "unexpected SSH channel message before {request} acknowledgement"
                )));
            }
        }
    }
}
/// Interactive channel. Process messages until EOF, including exit status and stderr.
pub struct SshShellStream {
    /// SSH channel used for input, output and window-change messages.
    pub channel: russh::Channel<client::Msg>,
    pending: VecDeque<russh::ChannelMsg>,
}
impl SshShellStream {
    /// Read the next event, including output buffered while awaiting shell setup.
    pub async fn wait(&mut self) -> Option<russh::ChannelMsg> {
        if let Some(message) = self.pending.pop_front() {
            Some(message)
        } else {
            self.channel.wait().await
        }
    }
}
