//! Redis actor: profile/credential I/O and all network operations stay off the UI thread.
use super::{
    Atom, Connection, Data, Error, Mutation, Profile, Result, ScanPage, Snapshot, copy_secret,
};
use nozhik_remote_ssh::{SshSession, credentials::Secret};
use std::{collections::HashMap, path::PathBuf, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

/// Typed Redis requests. A connection is always selected explicitly.
pub enum Action {
    /// Load public settings.
    Load,
    /// Save an expected profile; stale edits are rejected.
    Save {
        /// Expected persisted settings, absent for a new profile.
        original: Option<Profile>,
        /// Candidate public connection settings.
        profile: Profile,
        /// Session-only secret, never serialized.
        password: Option<Secret>,
    },
    /// Test or establish an authenticated endpoint.
    Connect {
        /// Candidate public connection settings.
        profile: Profile,
        /// Session-only secret, never serialized.
        password: Option<Secret>,
        /// Verified authenticated SSH session.
        ssh: Option<SshSession>,
        /// Test without installing a live connection.
        test: bool,
    },
    /// Close an endpoint and its tunnel.
    Disconnect(String),
    /// One cancellable key-name scan step.
    Scan {
        /// Selected connection name.
        name: String,
        /// Logical database index.
        database: u32,
        /// Server scan cursor.
        cursor: u64,
        /// Literal key-name substring.
        substring: String,
    },
    /// Read a selected key.
    Read {
        /// Selected connection name.
        name: String,
        /// Logical database index.
        database: u32,
        /// Original binary-safe key.
        key: Atom,
    },
    /// Apply a selected key's explicit operation.
    Mutate {
        /// Selected connection name.
        name: String,
        /// Logical database index.
        database: u32,
        /// Original consistent value for conflict detection.
        snapshot: Snapshot,
        /// Explicit requested write.
        mutation: Mutation,
    },
    /// Create a key without overwriting.
    Create {
        /// Selected connection name.
        name: String,
        /// Logical database index.
        database: u32,
        /// Original binary-safe key.
        key: Atom,
        /// Validated initial structure.
        data: Data,
    },
}
/// Worker result, correlated with the initiating UI request.
pub enum Output {
    /// Public profiles loaded or saved.
    Profiles(Vec<Profile>),
    /// Connection established or successfully tested.
    Connected {
        /// Selected connection name.
        name: String,
        /// Test without installing a live connection.
        test: bool,
    },
    /// Endpoint closed.
    Disconnected(String),
    /// Key-name cursor page.
    Page(ScanPage),
    /// Selected key's consistent baseline.
    Value(Snapshot),
    /// Explicit write completed; reload the affected key.
    Written,
}
/// Request identity, operation and cancellation ownership.
pub struct Request {
    /// Monotonic identity supplied by UI.
    pub id: u64,
    /// Requested action.
    pub action: Action,
    /// Read cancellation; sent writes may already have taken effect.
    pub cancel: CancellationToken,
}
/// Result envelope.
pub struct Reply {
    /// Initiating request identity.
    pub id: u64,
    /// Result or a user-visible error.
    pub result: Result<Output>,
}
/// Project-owned Redis worker with bounded queues.
pub struct Worker {
    input: mpsc::Sender<Request>,
    /// Results are drained by the UI.
    pub output: mpsc::Receiver<Reply>,
    stop: CancellationToken,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Worker {
    /// Start an isolated runtime; the callback wakes the native UI.
    #[must_use]
    pub fn start(path: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> Self {
        let (input, mut receiver) = mpsc::channel::<Request>(32);
        let (sender, output) = mpsc::channel(32);
        let stop = CancellationToken::new();
        let token = stop.clone();
        let thread = std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build();
            let Ok(runtime) = runtime else { return };
            runtime.block_on(async move {
                let mut actor = Actor { path, connections: HashMap::new(), passwords: HashMap::new(), tested: HashMap::new() };
                loop {
                    let request = tokio::select! { () = token.cancelled() => break, request = receiver.recv() => match request { Some(r) => r, None => break } };
                    let id = request.id;
                    let write = matches!(&request.action, Action::Mutate { .. } | Action::Create { .. } | Action::Save { .. });
                    let result = tokio::select! {
                        () = token.cancelled() => break,
                        () = request.cancel.cancelled(), if !write => Err(Error::Invalid("Redis operation cancelled".into())),
                        result = tokio::time::timeout(Duration::from_secs(45), actor.perform(request.action)) => result.unwrap_or_else(|_| Err(Error::Invalid(if write { "Redis write timed out; its outcome is unknown. Reload before retrying." } else { "Redis operation timed out" }.into())))
                    };
                    if sender.send(Reply { id, result }).await.is_err() { break; }
                    wake();
                }
            });
        });
        Self {
            input,
            output,
            stop,
            thread: Some(thread),
        }
    }
    /// Enqueue without blocking UI. False leaves the request unstarted.
    #[must_use]
    pub fn send(&self, request: Request) -> bool {
        self.input.try_send(request).is_ok()
    }
    /// Stop owned tasks and release all tunnels.
    pub fn shutdown(mut self) {
        self.stop.cancel();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        { /* Shutdown must not panic during project teardown. */
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
struct Actor {
    path: PathBuf,
    connections: HashMap<String, Connection>,
    passwords: HashMap<String, (Profile, Secret)>,
    tested: HashMap<String, (Profile, Secret)>,
}
impl Actor {
    fn connection(&self, name: &str) -> Result<&Connection> {
        self.connections
            .get(name)
            .ok_or_else(|| Error::Invalid("Redis is disconnected; reconnect first".into()))
    }
    #[allow(
        clippy::too_many_lines,
        reason = "Typed actor dispatch keeps endpoint and credential ownership in a single match"
    )]
    async fn perform(&mut self, action: Action) -> Result<Output> {
        match action {
            Action::Load => {
                let path = self.path.clone();
                Ok(Output::Profiles(
                    blocking(move || super::load_profiles(&path)).await?,
                ))
            }
            Action::Save {
                original,
                profile,
                password,
            } => self.save(original, profile, password).await,
            Action::Connect {
                profile,
                password,
                ssh,
                test,
            } => {
                let password = if let Some(secret) = password {
                    Some(secret)
                } else if let Some((_, secret)) = self
                    .passwords
                    .get(&profile.name)
                    .filter(|(p, _)| same_endpoint(p, &profile))
                {
                    Some(copy_secret(secret))
                } else if profile.save_password {
                    let p = profile.clone();
                    Some(blocking(move || Ok(Secret::new(p.credential()?.get_password()?))).await?)
                } else {
                    None
                };
                let connection =
                    Connection::connect(&profile, password.as_ref(), ssh.as_ref()).await?;
                let name = profile.name.clone();
                let profile_for_secret = profile.clone();
                if test {
                    self.tested.insert(
                        name.clone(),
                        (
                            profile,
                            password
                                .as_ref()
                                .map_or_else(|| Secret::new(String::new()), copy_secret),
                        ),
                    );
                } else {
                    self.connections.insert(name.clone(), connection);
                }
                if let Some(secret) = password {
                    self.passwords
                        .insert(name.clone(), (profile_for_secret, secret));
                }
                Ok(Output::Connected { name, test })
            }
            Action::Disconnect(name) => {
                self.connections.remove(&name);
                Ok(Output::Disconnected(name))
            }
            Action::Scan {
                name,
                database,
                cursor,
                substring,
            } => Ok(Output::Page(
                self.connection(&name)?
                    .scan(database, cursor, &substring)
                    .await?,
            )),
            Action::Read {
                name,
                database,
                key,
            } => Ok(Output::Value(
                self.connection(&name)?.read(database, &key).await?,
            )),
            Action::Mutate {
                name,
                database,
                snapshot,
                mutation,
            } => {
                self.connection(&name)?
                    .mutate(database, &snapshot, &mutation)
                    .await?;
                Ok(Output::Written)
            }
            Action::Create {
                name,
                database,
                key,
                data,
            } => {
                self.connection(&name)?
                    .create(database, &key, &data)
                    .await?;
                Ok(Output::Written)
            }
        }
    }
    async fn save(
        &mut self,
        original: Option<Profile>,
        profile: Profile,
        password: Option<Secret>,
    ) -> Result<Output> {
        profile.validate()?;
        if profile.save_password
            && !self.tested.get(&profile.name).is_some_and(|(p, s)| {
                *p == profile
                    && password
                        .as_ref()
                        .is_some_and(|secret| secret.expose() == s.expose())
            })
        {
            return Err(Error::Invalid(
                "Test this connection and password before saving protected credentials".into(),
            ));
        }
        let path = self.path.clone();
        let candidate = profile.clone();
        let previous = original.clone();
        let saved_secret = password.as_ref().map(copy_secret);
        let profiles = blocking(move || {
            let mut profiles = super::load_profiles(&path)?;
            let index = if let Some(old) = &previous {
                Some(profiles.iter().position(|p| p == old).ok_or_else(|| {
                    Error::Invalid("Redis settings changed; reopen Edit Connection".into())
                })?)
            } else {
                None
            };
            if profiles
                .iter()
                .enumerate()
                .any(|(i, p)| p.name == candidate.name && Some(i) != index)
            {
                return Err(Error::Invalid(
                    "Redis connection name already exists".into(),
                ));
            }
            if candidate.save_password
                && let Some(secret) = saved_secret
            {
                candidate.credential()?.set_password(secret.expose())?;
            }
            if let Some(index) = index {
                profiles[index] = candidate;
            } else {
                profiles.push(candidate);
            }
            super::save_profiles(&path, &profiles)?;
            Ok(profiles)
        })
        .await?;
        if let Some(old) = original {
            self.connections.remove(&old.name);
            self.passwords.remove(&old.name);
        }
        self.connections.remove(&profile.name);
        if let Some(secret) = password {
            self.passwords
                .insert(profile.name.clone(), (profile.clone(), secret));
        }
        self.tested.remove(&profile.name);
        Ok(Output::Profiles(profiles))
    }
}
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T> + Send + 'static) -> Result<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| Error::Invalid(e.to_string()))?
}

fn same_endpoint(left: &Profile, right: &Profile) -> bool {
    left.host == right.host
        && left.port == right.port
        && left.user == right.user
        && left.database == right.database
        && left.ssh_profile_id == right.ssh_profile_id
}
