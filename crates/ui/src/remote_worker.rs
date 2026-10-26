//! Serialized remote operations on an owned runtime; UI only handles completions.
use crate::remote_shell::RemoteShell;
use nozhik_remote_ssh::{
    SshProfile, SshSession,
    credentials::{CredentialStore, OsCredentialStore, Secret, same_identity},
    sftp::{RemoteFileEntry, SftpClient},
    tunnel::TunnelHandle,
};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
use tokio::sync::mpsc;

#[derive(Clone, Copy)]
pub enum Intent {
    Test,
    Database(u64),
    Terminal,
    Browse,
    Tunnels,
}
pub enum Command {
    Verify(SshProfile, Intent),
    Trust(SshProfile, String, Intent),
    Connect(SshProfile, Option<Secret>, Intent),
    Disconnect(String),
    List(String, String, bool),
    Read(String, String),
    Write(String, String, PathBuf, u64, ropey::Rope),
    UpsertProfile(Option<String>, SshProfile),
    DeleteProfile(String),
    ForgetPassword(String),
    StopTunnels(String),
}
pub enum Reply {
    TrustRequired(SshProfile, String, Intent),
    HostKeyChanged(String),
    Verified(SshProfile, Intent),
    Profiles(Vec<SshProfile>),
    ProfilesChanged(Vec<SshProfile>),
    CredentialsRequired(SshProfile, Intent, String),
    Connected(String),
    DatabaseSession(u64, SshSession),
    DatabaseFailed(u64, String),
    Disconnected(String),
    Shell(String, RemoteShell),
    Listed(String, String, Vec<RemoteFileEntry>),
    Read(String, String, PathBuf, String),
    Saved(PathBuf, u64),
    Message(String),
    Error(String),
}
pub struct Worker {
    input: mpsc::Sender<Command>,
    pub output: mpsc::Receiver<Reply>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}
struct State {
    sessions: HashMap<String, SshSession>,
    profiles: HashMap<String, SshProfile>,
    sftp: HashMap<String, SftpClient>,
    tunnels: HashMap<String, Vec<TunnelHandle>>,
    cache: tempfile::TempDir,
    documents: HashMap<(String, String), PathBuf>,
    path: PathBuf,
    credentials: Arc<dyn CredentialStore>,
    next_document: u64,
}
impl Worker {
    pub fn start(path: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (input, mut commands) = mpsc::channel(64);
        let (sender, output) = mpsc::channel(64);
        let (stop, stopped) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _sent = sender.blocking_send(Reply::Error(e.to_string()));
                    wake();
                    return;
                }
            };
            runtime.block_on(async {
                let run = async {
                    let cache = tokio::task::spawn_blocking(tempfile::tempdir).await.map_err(|e| join_error(&e))??;
                    let mut state = State { sessions: HashMap::new(), profiles: HashMap::new(), sftp: HashMap::new(), tunnels: HashMap::new(), cache, documents: HashMap::new(), path, credentials: Arc::new(OsCredentialStore), next_document: 1 };
                    let profile_path = state.path.clone();
                    let loaded = match tokio::task::spawn_blocking(move || nozhik_remote_ssh::load_profiles(&profile_path)).await.map_err(|e| join_error(&e))? {
                        Ok(profiles) => Reply::Profiles(profiles), Err(e) => Reply::Error(format!("SSH profiles: {e}")),
                    };
                    let _sent = sender.send(loaded).await; wake();
                    let mut timer = tokio::time::interval(std::time::Duration::from_secs(1));
                    loop {
                        tokio::select! {
                            command = commands.recv() => {
                                let Some(command) = command else { break; };
                                let database_request = match &command {
                                    Command::Verify(_, Intent::Database(id)) | Command::Trust(_, _, Intent::Database(id)) | Command::Connect(_, _, Intent::Database(id)) => Some(*id), _ => None,
                                };
                                let refresh_profiles = matches!(&command, Command::Connect(..));
                                let result = tokio::time::timeout(std::time::Duration::from_secs(35), state.execute(command)).await;
                                let reply = match result {
                                    Ok(Ok(reply)) => reply,
                                    Ok(Err(e @ nozhik_remote_ssh::Error::HostKeyChanged { .. })) => Reply::HostKeyChanged(e.to_string()),
                                    Ok(Err(e)) => Reply::Error(e.to_string()),
                                    Err(e) => Reply::Error(e.to_string()),
                                };
                                if let Some(id) = database_request && let Reply::Error(error) | Reply::HostKeyChanged(error) = &reply {
                                    if sender.send(Reply::DatabaseFailed(id, error.clone())).await.is_err() { break; } wake();
                                }
                                if refresh_profiles && let Ok(profiles) = state.saved_profiles().await {
                                    if sender.send(Reply::Profiles(profiles)).await.is_err() { break; } wake();
                                }
                                for name in state.sessions.keys() {
                                    if sender.send(Reply::Connected(name.clone())).await.is_err() { break; }
                                    wake();
                                }
                                if sender.send(reply).await.is_err() { break; } wake();
                            }
                            _ = timer.tick() => {
                                let mut lost = Vec::new();
                                for (name, session) in &state.sessions {
                                    if !session.is_connected().await { lost.push(name.clone()); }
                                }
                                for name in lost {
                                    state.remove(&name);
                                    if sender.send(Reply::Disconnected(name)).await.is_err() { break; } wake();
                                }
                            }
                        }
                    }
                    Ok::<_, nozhik_remote_ssh::Error>(())
                };
                tokio::select! {
                    result = run => { if let Err(e) = result { let _sent = sender.send(Reply::Error(e.to_string())).await; wake(); } }
                    _ = stopped => {}
                }
            });
        });
        Self {
            input,
            output,
            stop: Some(stop),
        }
    }
    pub fn send(&self, command: Command) -> bool {
        self.input.try_send(command).is_ok()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _stopped = stop.send(());
        }
    }
}
impl State {
    fn remove(&mut self, name: &str) {
        self.tunnels.remove(name);
        self.sftp.remove(name);
        self.sessions.remove(name);
        self.profiles.remove(name);
    }
    async fn files(&mut self, name: &str) -> nozhik_remote_ssh::Result<&mut SftpClient> {
        if !self.sftp.contains_key(name) {
            let session = self
                .sessions
                .get(name)
                .ok_or_else(|| nozhik_remote_ssh::Error::Invalid("SSH disconnected".into()))?;
            self.sftp
                .insert(name.into(), SftpClient::new(session).await?);
        }
        self.sftp
            .get_mut(name)
            .ok_or_else(|| nozhik_remote_ssh::Error::Invalid("SFTP unavailable".into()))
    }
    async fn resolve_profile(&self, profile: SshProfile) -> nozhik_remote_ssh::Result<SshProfile> {
        let path = self.path.with_file_name("ssh_known_hosts.json");
        tokio::task::spawn_blocking(move || nozhik_remote_ssh::trust::resolve(&path, profile))
            .await
            .map_err(|e| join_error(&e))?
    }
    async fn saved_profiles(&self) -> nozhik_remote_ssh::Result<Vec<SshProfile>> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || nozhik_remote_ssh::load_profiles(&path))
            .await
            .map_err(|e| join_error(&e))?
    }
    async fn persist_profiles(
        &self,
        profiles: Vec<SshProfile>,
    ) -> nozhik_remote_ssh::Result<Vec<SshProfile>> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            nozhik_remote_ssh::save_profiles(&path, &profiles)?;
            Ok(profiles)
        })
        .await
        .map_err(|e| join_error(&e))?
    }
    async fn forget_secret(&self, profile: SshProfile) -> nozhik_remote_ssh::Result<()> {
        let credentials = self.credentials.clone();
        tokio::task::spawn_blocking(move || credentials.delete(&profile))
            .await
            .map_err(|e| join_error(&e))?
    }
    async fn invalidate_profile(&mut self, name: &str) {
        if let Some(session) = self.sessions.get(name) {
            // Profile changes must also close shells that still own cloned channel handles.
            if let Err(error) = session.disconnect().await {
                eprintln!("SSH disconnect during profile update: {error}");
            }
        }
        self.remove(name);
        self.documents.retain(|(host, _), _| host != name);
    }
    async fn upsert_profile(
        &mut self,
        original: Option<String>,
        mut profile: SshProfile,
    ) -> nozhik_remote_ssh::Result<Reply> {
        profile.validate()?;
        let mut profiles = self.saved_profiles().await?;
        let previous = if let Some(name) = &original {
            Some(
                profiles
                    .iter()
                    .find(|p| &p.name == name)
                    .cloned()
                    .ok_or_else(|| {
                        nozhik_remote_ssh::Error::Invalid("SSH profile no longer exists".into())
                    })?,
            )
        } else {
            None
        };
        if profiles
            .iter()
            .any(|p| p.name == profile.name && Some(&p.name) != original.as_ref())
        {
            return Err(nozhik_remote_ssh::Error::Invalid(
                "Profile name already exists".into(),
            ));
        }
        if let Some(old) = &previous {
            if !same_identity(old, &profile) {
                profile.save_password = false;
                if old.host != profile.host || old.port != profile.port {
                    profile.host_key_sha256.clear();
                }
            }
            if old.save_password && (!profile.save_password || !same_identity(old, &profile)) {
                self.forget_secret(old.clone()).await?;
            }
        }
        if let Some(name) = &original {
            profiles.retain(|p| &p.name != name);
        }
        profiles.push(profile.clone());
        let profiles = self.persist_profiles(profiles).await?;
        if let Some(old) = previous {
            if !same_identity(&old, &profile)
                || old.host_key_sha256 != profile.host_key_sha256
                || old.forwards != profile.forwards
            {
                self.invalidate_profile(&old.name).await;
            }
        } else {
            // A successful unsaved Test Connection may already own this name.
            self.invalidate_profile(&profile.name).await;
        }
        Ok(Reply::ProfilesChanged(profiles))
    }
    async fn delete_profile(&mut self, name: &str) -> nozhik_remote_ssh::Result<Reply> {
        let mut profiles = self.saved_profiles().await?;
        let previous = profiles
            .iter()
            .find(|p| p.name == name)
            .cloned()
            .ok_or_else(|| {
                nozhik_remote_ssh::Error::Invalid("SSH profile no longer exists".into())
            })?;
        if previous.save_password {
            self.forget_secret(previous).await?;
        }
        profiles.retain(|p| p.name != name);
        let profiles = self.persist_profiles(profiles).await?;
        self.invalidate_profile(name).await;
        Ok(Reply::ProfilesChanged(profiles))
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Authentication and its explicit credential-persistence acknowledgement share a single lifecycle"
    )]
    async fn connect(
        &mut self,
        profile: SshProfile,
        secret: Option<Secret>,
        intent: Intent,
    ) -> nozhik_remote_ssh::Result<Reply> {
        let profile = self.resolve_profile(profile).await?;
        if self
            .saved_profiles()
            .await?
            .iter()
            .any(|saved| saved.name == profile.name && !same_identity(saved, &profile))
        {
            return Err(nozhik_remote_ssh::Error::Invalid("Save changed SSH settings before connecting; open documents still belong to the previous host".into()));
        }
        if let Some(existing) = self.profiles.get(&profile.name)
            && (!same_identity(existing, &profile)
                || existing.host_key_sha256 != profile.host_key_sha256)
        {
            return Err(nozhik_remote_ssh::Error::Invalid(
                "Disconnect this profile before changing its connection settings".into(),
            ));
        }
        if let Some(session) = self.sessions.get(&profile.name)
            && !session.is_connected().await
        {
            self.remove(&profile.name);
        }
        if !self.sessions.contains_key(&profile.name) {
            let supplied = secret.is_some();
            let secret = if !supplied && profile.save_password {
                let credentials = self.credentials.clone();
                let selected = profile.clone();
                match tokio::task::spawn_blocking(move || credentials.load(&selected))
                    .await
                    .map_err(|e| join_error(&e))?
                {
                    Ok(secret) => secret,
                    Err(error) => {
                        let mut profile = profile;
                        profile.save_password = false;
                        return Ok(Reply::CredentialsRequired(
                            profile,
                            intent,
                            format!("Saved password could not be read: {error}"),
                        ));
                    }
                }
            } else {
                secret
            };
            if secret.is_none() && !matches!(profile.auth, nozhik_remote_ssh::AuthMethod::Agent) {
                return Ok(Reply::CredentialsRequired(profile, intent, String::new()));
            }
            let session =
                match SshSession::connect(&profile, secret.as_ref().map(Secret::expose)).await {
                    Ok(session) => session,
                    Err(nozhik_remote_ssh::Error::AuthenticationRejected) if !supplied => {
                        return Ok(Reply::CredentialsRequired(
                            profile,
                            intent,
                            "Saved password was rejected. Enter a new password.".into(),
                        ));
                    }
                    Err(error) => return Err(error),
                };
            if supplied && !matches!(profile.auth, nozhik_remote_ssh::AuthMethod::Agent) {
                let mut profiles = self.saved_profiles().await?;
                if let Some(saved) = profiles
                    .iter_mut()
                    .find(|saved| same_identity(saved, &profile))
                {
                    if profile.save_password {
                        let credentials = self.credentials.clone();
                        let selected = profile.clone();
                        if let Some(secret) = secret {
                            tokio::task::spawn_blocking(move || {
                                credentials.save(&selected, &secret)
                            })
                            .await
                            .map_err(|e| join_error(&e))??;
                        }
                    } else if saved.save_password {
                        self.forget_secret(saved.clone()).await?;
                    }
                    saved.save_password = profile.save_password;
                    self.persist_profiles(profiles).await?;
                } else if profile.save_password {
                    return Err(nozhik_remote_ssh::Error::Invalid(
                        "Save the SSH profile before remembering its password".into(),
                    ));
                }
            }
            self.sessions.insert(profile.name.clone(), session);
            self.profiles.insert(profile.name.clone(), profile.clone());
        }
        let session = self
            .sessions
            .get(&profile.name)
            .ok_or_else(|| nozhik_remote_ssh::Error::Invalid("SSH disconnected".into()))?
            .clone();
        match intent {
            Intent::Database(id) => Ok(Reply::DatabaseSession(id, session)),
            Intent::Test => Ok(Reply::Connected(profile.name)),
            Intent::Terminal => Ok(Reply::Shell(
                profile.name,
                RemoteShell::start(session.open_shell(24, 80).await?),
            )),
            Intent::Browse => {
                let entries = self.files(&profile.name).await?.list_dir(".").await?;
                Ok(Reply::Listed(profile.name, ".".into(), entries))
            }
            Intent::Tunnels => {
                if self.tunnels.contains_key(&profile.name) {
                    return Ok(Reply::Message(
                        "Tunnels already running; stop them before restarting".into(),
                    ));
                }
                let mut handles = Vec::new();
                let mut addresses = Vec::new();
                for forward in &profile.forwards {
                    let handle = session
                        .start_tunnel(
                            forward.local_port,
                            &forward.remote_host,
                            forward.remote_port,
                        )
                        .await?;
                    addresses.push(format!(
                        "{} → {}:{}",
                        handle.local_addr(),
                        forward.remote_host,
                        forward.remote_port
                    ));
                    handles.push(handle);
                }
                self.tunnels.insert(profile.name.clone(), handles);
                Ok(Reply::Message(format!(
                    "{}: {}",
                    profile.name,
                    addresses.join(", ")
                )))
            }
        }
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Exhaustive typed command routing keeps remote state on one worker"
    )]
    async fn execute(&mut self, command: Command) -> nozhik_remote_ssh::Result<Reply> {
        match command {
            Command::Verify(profile, intent) => {
                let profile = self.resolve_profile(profile).await?;
                match SshSession::verify_host_key(&profile).await {
                    Ok(()) => Ok(Reply::Verified(profile, intent)),
                    Err(nozhik_remote_ssh::Error::UnknownHostKey(pin)) => {
                        Ok(Reply::TrustRequired(profile, pin, intent))
                    }
                    Err(e) => Err(e),
                }
            }
            Command::Trust(profile, pin, intent) => {
                let path = self.path.with_file_name("ssh_known_hosts.json");
                let profile = tokio::task::spawn_blocking(move || {
                    nozhik_remote_ssh::trust::remember(&path, &profile, &pin)?;
                    nozhik_remote_ssh::trust::resolve(&path, profile)
                })
                .await
                .map_err(|e| join_error(&e))??;
                Ok(Reply::Verified(profile, intent))
            }
            Command::UpsertProfile(original, profile) => {
                self.upsert_profile(original, profile).await
            }
            Command::DeleteProfile(name) => self.delete_profile(&name).await,
            Command::ForgetPassword(name) => {
                let mut profile = self
                    .saved_profiles()
                    .await?
                    .into_iter()
                    .find(|p| p.name == name)
                    .ok_or_else(|| {
                        nozhik_remote_ssh::Error::Invalid("SSH profile no longer exists".into())
                    })?;
                profile.save_password = false;
                self.upsert_profile(Some(name), profile).await
            }
            Command::Connect(profile, secret, intent) => {
                self.connect(profile, secret, intent).await
            }
            Command::Disconnect(name) => {
                if let Some(session) = self.sessions.get(&name) {
                    session.disconnect().await?;
                }
                self.remove(&name);
                Ok(Reply::Disconnected(name))
            }
            Command::StopTunnels(name) => {
                self.tunnels.remove(&name);
                Ok(Reply::Message(format!("{name}: tunnels stopped")))
            }
            Command::List(name, path, refresh) => {
                let sftp = self.files(&name).await?;
                if refresh {
                    sftp.invalidate();
                }
                let entries = sftp.list_dir(&path).await?;
                Ok(Reply::Listed(name, path, entries))
            }
            Command::Read(name, path) => {
                let bytes = self.files(&name).await?.read_file(&path).await?;
                if bytes.iter().take(1024).any(|b| *b == 0) {
                    return Err(nozhik_remote_ssh::Error::Invalid(
                        "Binary file not supported".into(),
                    ));
                }
                let text = String::from_utf8(bytes)
                    .map_err(|e| nozhik_remote_ssh::Error::Invalid(e.to_string()))?;
                let identity = (name.clone(), path.clone());
                let cache_path = if let Some(path) = self.documents.get(&identity) {
                    path.clone()
                } else {
                    let path = self
                        .cache
                        .path()
                        .join(format!("document-{}", self.next_document));
                    self.next_document = self.next_document.checked_add(1).ok_or_else(|| {
                        nozhik_remote_ssh::Error::Invalid("remote document IDs exhausted".into())
                    })?;
                    self.documents.insert(identity, path.clone());
                    path
                };
                tokio::fs::write(&cache_path, &text).await?;
                Ok(Reply::Read(name, path, cache_path, text))
            }
            Command::Write(name, path, cache, revision, text) => {
                let text = text.to_string();
                self.files(&name)
                    .await?
                    .write_file(&path, text.as_bytes())
                    .await?;
                tokio::fs::write(&cache, &text).await?;
                Ok(Reply::Saved(cache, revision))
            }
        }
    }
}

fn join_error(error: &tokio::task::JoinError) -> nozhik_remote_ssh::Error {
    nozhik_remote_ssh::Error::Invalid(format!("SSH filesystem worker: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use nozhik_remote_ssh::{AuthMethod, PortForward};
    use std::sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    };
    #[derive(Default)]
    struct MemoryCredentials {
        items: Mutex<HashMap<String, String>>,
        deny: AtomicBool,
    }
    impl CredentialStore for MemoryCredentials {
        fn load(&self, profile: &SshProfile) -> nozhik_remote_ssh::Result<Option<Secret>> {
            if self.deny.load(Ordering::Relaxed) {
                return Err(nozhik_remote_ssh::Error::Invalid(
                    "credential store locked".into(),
                ));
            }
            Ok(self
                .items
                .lock()
                .map_err(|_| nozhik_remote_ssh::Error::Invalid("poisoned mock".into()))?
                .get(&profile.name)
                .cloned()
                .map(Secret::new))
        }
        fn save(&self, profile: &SshProfile, secret: &Secret) -> nozhik_remote_ssh::Result<()> {
            if self.deny.load(Ordering::Relaxed) {
                return Err(nozhik_remote_ssh::Error::Invalid(
                    "credential store locked".into(),
                ));
            }
            self.items
                .lock()
                .map_err(|_| nozhik_remote_ssh::Error::Invalid("poisoned mock".into()))?
                .insert(profile.name.clone(), secret.expose().into());
            Ok(())
        }
        fn delete(&self, profile: &SshProfile) -> nozhik_remote_ssh::Result<()> {
            if self.deny.load(Ordering::Relaxed) {
                return Err(nozhik_remote_ssh::Error::Invalid(
                    "credential store locked".into(),
                ));
            }
            self.items
                .lock()
                .map_err(|_| nozhik_remote_ssh::Error::Invalid("poisoned mock".into()))?
                .remove(&profile.name);
            Ok(())
        }
    }
    fn profile(name: &str) -> SshProfile {
        SshProfile {
            name: name.into(),
            host: "127.0.0.1".into(),
            port: 22,
            username: "user".into(),
            auth: AuthMethod::Password,
            save_password: false,
            host_key_sha256: String::new(),
            forwards: Vec::new(),
        }
    }
    fn test_state(
        directory: &std::path::Path,
        credentials: Arc<dyn CredentialStore>,
    ) -> std::io::Result<State> {
        Ok(State {
            sessions: HashMap::new(),
            profiles: HashMap::new(),
            sftp: HashMap::new(),
            tunnels: HashMap::new(),
            cache: tempfile::tempdir()?,
            documents: HashMap::new(),
            path: directory.join("ssh_hosts.json"),
            credentials,
            next_document: 1,
        })
    }
    #[tokio::test]
    async fn saved_password_reuse_and_rejected_or_locked_credentials()
    -> Result<(), Box<dyn std::error::Error>> {
        struct Server;
        impl russh::server::Handler for Server {
            type Error = russh::Error;
            fn auth_password(
                &mut self,
                _user: &str,
                password: &str,
            ) -> impl Future<Output = Result<russh::server::Auth, Self::Error>> + Send {
                std::future::ready(Ok(if password == "valid-password" {
                    russh::server::Auth::Accept
                } else {
                    russh::server::Auth::Reject {
                        proceed_with_methods: None,
                        partial_success: false,
                    }
                }))
            }
        }
        tokio::time::timeout(std::time::Duration::from_secs(5),async {
            let key = russh::keys::PrivateKey::random(&mut rand::rng(),russh::keys::Algorithm::Ed25519)?;
            let pin = key.public_key().fingerprint(russh::keys::HashAlg::Sha256).to_string();
            let config = Arc::new(russh::server::Config { keys: vec![key], auth_rejection_time: std::time::Duration::ZERO, ..Default::default() });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?; let port = listener.local_addr()?.port();
            let task = tokio::spawn(async move {
                let mut tasks = tokio::task::JoinSet::new();
                while let Ok((socket,_)) = listener.accept().await {
                    let config = config.clone();
                    tasks.spawn(async move { russh::server::run_stream(config,socket,Server).await?.await });
                }
            });
            let dir = tempfile::tempdir()?; let credentials = Arc::new(MemoryCredentials::default());
            let mut state = test_state(dir.path(), credentials.clone())?;
            let mut selected = SshProfile { port, host_key_sha256: pin, ..profile("saved") };
            state.upsert_profile(None,selected.clone()).await?;
            selected.save_password = true;
            assert!(matches!(state.connect(selected.clone(),Some(Secret::new("valid-password".into())),Intent::Test).await?,Reply::Connected(_)));
            assert_eq!(credentials.load(&selected)?.ok_or("missing saved password")?.expose(),"valid-password");
            assert!(state.saved_profiles().await?.first().is_some_and(|p| p.save_password));
            assert!(!std::fs::read_to_string(&state.path)?.contains("valid-password"));
            state.invalidate_profile("saved").await;
            assert!(matches!(state.connect(selected.clone(),None,Intent::Test).await?,Reply::Connected(_)));
            state.invalidate_profile("saved").await;
            credentials.save(&selected,&Secret::new("wrong".into()))?;
            assert!(matches!(state.connect(selected.clone(),None,Intent::Test).await?,Reply::CredentialsRequired(_,_,message) if message.contains("rejected")));
            credentials.deny.store(true,Ordering::Relaxed);
            assert!(matches!(state.connect(selected.clone(),None,Intent::Test).await?,Reply::CredentialsRequired(p,_,message) if !p.save_password && message.contains("could not be read")));
            credentials.deny.store(false,Ordering::Relaxed);
            credentials.delete(&selected)?;
            assert!(matches!(state.connect(selected,None,Intent::Test).await?,Reply::CredentialsRequired(..)));
            task.abort();
            Ok::<_,Box<dyn std::error::Error>>(())
        }).await??;
        Ok(())
    }
    #[tokio::test]
    async fn edit_delete_forget_and_locked_store_preserve_config_and_document_identity()
    -> Result<(), Box<dyn std::error::Error>> {
        let dir = tempfile::tempdir()?;
        let credentials = Arc::new(MemoryCredentials::default());
        let mut state = test_state(dir.path(), credentials.clone())?;
        let mut original = profile("server");
        original.save_password = true;
        state.upsert_profile(None, original.clone()).await?;
        credentials.save(&original, &Secret::new("private-credential".into()))?;
        state.documents.insert(
            (original.name.clone(), "./file".into()),
            state.cache.path().join("old"),
        );
        let mut edited = original.clone();
        edited.host = "new.example".into();
        edited.forwards = vec![
            PortForward {
                local_port: 15432,
                remote_host: "localhost".into(),
                remote_port: 5432,
            },
            PortForward {
                local_port: 13306,
                remote_host: "localhost".into(),
                remote_port: 3306,
            },
        ];
        let error = state
            .connect(edited.clone(), None, Intent::Test)
            .await
            .err()
            .ok_or("unsaved edits could retarget a saved host")?;
        assert!(error.to_string().contains("Save changed SSH settings"));
        credentials.deny.store(true, Ordering::Relaxed);
        assert!(
            state
                .upsert_profile(Some(original.name.clone()), edited.clone())
                .await
                .is_err()
        );
        assert_eq!(state.saved_profiles().await?, vec![original.clone()]);
        credentials.deny.store(false, Ordering::Relaxed);
        state
            .upsert_profile(Some(original.name.clone()), edited.clone())
            .await?;
        let stored = state
            .saved_profiles()
            .await?
            .pop()
            .ok_or("missing profile")?;
        assert_eq!(stored.host, edited.host);
        assert!(!stored.save_password);
        assert_eq!(stored.forwards.len(), 2);
        assert!(credentials.load(&original)?.is_none());
        assert_eq!(state.documents.len(), 0);
        assert!(!std::fs::read_to_string(&state.path)?.contains("private-credential"));
        let mut renamed = stored;
        renamed.name = "renamed".into();
        state
            .upsert_profile(Some("server".into()), renamed.clone())
            .await?;
        state.upsert_profile(None, profile("other")).await?;
        assert!(
            state
                .upsert_profile(Some("renamed".into()), profile("other"))
                .await
                .is_err()
        );
        assert!(
            state
                .upsert_profile(Some("missing".into()), profile("third"))
                .await
                .is_err()
        );
        renamed.save_password = true;
        state
            .upsert_profile(Some("renamed".into()), renamed.clone())
            .await?;
        credentials.save(&renamed, &Secret::new("private-credential".into()))?;
        state
            .execute(Command::ForgetPassword("renamed".into()))
            .await?;
        assert!(credentials.load(&renamed)?.is_none());
        assert!(
            state
                .saved_profiles()
                .await?
                .iter()
                .all(|p| !p.save_password)
        );
        state.delete_profile("renamed").await?;
        assert_eq!(state.saved_profiles().await?, vec![profile("other")]);
        state.delete_profile("other").await?;
        assert_eq!(state.saved_profiles().await?, [] as [SshProfile; 0]);
        Ok(())
    }
}
