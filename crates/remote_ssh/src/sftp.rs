//! SFTP v3 operations with lazy, bounded, expiring directory snapshots.
use crate::{OPERATION_TIMEOUT, Result, SshSession};
use russh_sftp::{client::SftpSession, protocol::FileAttributes};
use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

/// Remote metadata. Missing server attributes remain unknown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteFileEntry {
    /// Basename supplied by the server.
    pub name: String,
    /// Size in bytes, if supplied.
    pub size: Option<u64>,
    /// POSIX mode including file type bits.
    pub permissions: Option<u32>,
    /// Directory flag; symbolic links are not traversed as directories.
    pub is_dir: bool,
    /// Symbolic link flag.
    pub is_symlink: bool,
    /// Unix modification timestamp in seconds.
    pub modified: Option<u32>,
}
impl RemoteFileEntry {
    /// Convert SFTP attributes without fabricating missing values.
    #[must_use]
    pub fn from_attributes(name: String, attributes: &FileAttributes) -> Self {
        Self {
            name,
            size: attributes.size,
            permissions: attributes.permissions,
            is_dir: attributes.is_dir(),
            is_symlink: attributes.file_type().is_symlink(),
            modified: attributes.mtime,
        }
    }
}
/// One SFTP subsystem and a cache of at most 128 directory listings for 30 seconds.
pub struct SftpClient {
    session: SftpSession,
    cache: HashMap<String, (Instant, Vec<RemoteFileEntry>)>,
}
impl SftpClient {
    /// Open the SFTP subsystem on an existing authenticated transport.
    /// # Errors
    /// Returns channel/subsystem rejection, SFTP negotiation or deadline errors.
    pub async fn new(ssh: &SshSession) -> Result<Self> {
        tokio::time::timeout(OPERATION_TIMEOUT, async {
            let mut channel = ssh.handle.lock().await.channel_open_session().await?;
            channel.request_subsystem(true, "sftp").await?;
            loop {
                match channel.wait().await {
                    Some(russh::ChannelMsg::Success) => break,
                    Some(russh::ChannelMsg::WindowAdjusted { .. }) => {}
                    Some(russh::ChannelMsg::Failure) => {
                        return Err(crate::Error::Invalid(
                            "server rejected SFTP subsystem".into(),
                        ));
                    }
                    _ => {
                        return Err(crate::Error::Invalid(
                            "unexpected message/closed channel before SFTP acknowledgement".into(),
                        ));
                    }
                }
            }
            let session = SftpSession::new(channel.into_stream()).await?;
            session.set_timeout(15);
            Ok(Self {
                session,
                cache: HashMap::new(),
            })
        })
        .await?
    }
    /// Discard cached listings, for explicit refresh or external changes.
    pub fn invalidate(&mut self) {
        self.cache.clear();
    }
    /// Fetch immediate children on demand, reusing fresh cached listings.
    /// # Errors
    /// Returns SFTP permission/transport/deadline errors.
    pub async fn list_dir(&mut self, path: &str) -> Result<Vec<RemoteFileEntry>> {
        if let Some((time, entries)) = self.cache.get(path)
            && time.elapsed() < Duration::from_secs(30)
        {
            return Ok(entries.clone());
        }
        let listing =
            tokio::time::timeout(OPERATION_TIMEOUT, self.session.read_dir(path)).await??;
        let mut entries: Vec<_> = listing
            .filter_map(|entry| {
                let name = entry.file_name();
                if name == "." || name == ".." || name.contains('/') || name.contains('\0') {
                    return None;
                }
                Some(RemoteFileEntry::from_attributes(name, &entry.metadata()))
            })
            .collect();
        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
        if self.cache.len() >= 128 {
            self.cache.clear();
        }
        self.cache
            .insert(path.into(), (Instant::now(), entries.clone()));
        Ok(entries)
    }
    /// Download a file; editor limit is 16 MiB, enforced while streaming.
    /// # Errors
    /// Returns oversized file, SFTP, I/O or deadline failures.
    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        use tokio::io::AsyncReadExt;
        tokio::time::timeout(OPERATION_TIMEOUT, async {
            let mut file = self.session.open(path).await?.take(16 * 1024 * 1024 + 1);
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes).await?;
            if bytes.len() > 16 * 1024 * 1024 {
                return Err(crate::Error::Invalid(
                    "remote editor file exceeds 16 MiB".into(),
                ));
            }
            Ok(bytes)
        })
        .await?
    }
    /// Upload a file. Failure leaves editor dirty; server writes are not atomic.
    /// # Errors
    /// Returns remote permission, transport or deadline failures.
    pub async fn write_file(&mut self, path: &str, data: &[u8]) -> Result<()> {
        tokio::time::timeout(OPERATION_TIMEOUT, async {
            use russh_sftp::protocol::OpenFlags;
            use tokio::io::AsyncWriteExt;
            let mut file = self
                .session
                .open_with_flags(
                    path,
                    OpenFlags::WRITE | OpenFlags::CREATE | OpenFlags::TRUNCATE,
                )
                .await?;
            file.write_all(data).await?;
            file.close().await?;
            Ok::<_, crate::Error>(())
        })
        .await??;
        self.invalidate();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use russh_sftp::protocol::{Data, File, Handle, Name, OpenFlags, Status, StatusCode};
    use std::{
        future::{Future, ready},
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };
    struct MemoryFs {
        bytes: Vec<u8>,
        listed: bool,
        requests: Arc<AtomicUsize>,
    }
    fn ok_status(id: u32) -> Status {
        Status {
            id,
            status_code: StatusCode::Ok,
            error_message: String::new(),
            language_tag: String::new(),
        }
    }
    impl russh_sftp::server::Handler for MemoryFs {
        type Error = StatusCode;
        fn unimplemented(&self) -> StatusCode {
            StatusCode::OpUnsupported
        }
        fn open(
            &mut self,
            id: u32,
            filename: String,
            flags: OpenFlags,
            _attrs: FileAttributes,
        ) -> impl Future<Output = std::result::Result<Handle, StatusCode>> + Send {
            if flags.contains(OpenFlags::TRUNCATE) {
                self.bytes.clear();
            }
            ready(Ok(Handle {
                id,
                handle: filename,
            }))
        }
        fn close(
            &mut self,
            id: u32,
            _handle: String,
        ) -> impl Future<Output = std::result::Result<Status, StatusCode>> + Send {
            ready(Ok(ok_status(id)))
        }
        fn read(
            &mut self,
            id: u32,
            _handle: String,
            offset: u64,
            len: u32,
        ) -> impl Future<Output = std::result::Result<Data, StatusCode>> + Send {
            let result = usize::try_from(offset)
                .ok()
                .and_then(|offset| self.bytes.get(offset..))
                .filter(|bytes| !bytes.is_empty())
                .map(|bytes| Data {
                    id,
                    data: bytes.iter().copied().take(len as usize).collect(),
                })
                .ok_or(StatusCode::Eof);
            ready(result)
        }
        fn write(
            &mut self,
            id: u32,
            _handle: String,
            offset: u64,
            data: Vec<u8>,
        ) -> impl Future<Output = std::result::Result<Status, StatusCode>> + Send {
            if offset != self.bytes.len() as u64 {
                return ready(Err(StatusCode::Failure));
            }
            self.bytes.extend(data);
            ready(Ok(ok_status(id)))
        }
        fn opendir(
            &mut self,
            id: u32,
            path: String,
        ) -> impl Future<Output = std::result::Result<Handle, StatusCode>> + Send {
            self.listed = false;
            self.requests.fetch_add(1, Ordering::Relaxed);
            ready(Ok(Handle { id, handle: path }))
        }
        fn readdir(
            &mut self,
            id: u32,
            _handle: String,
        ) -> impl Future<Output = std::result::Result<Name, StatusCode>> + Send {
            if self.listed {
                return ready(Err(StatusCode::Eof));
            }
            self.listed = true;
            ready(Ok(Name {
                id,
                files: vec![
                    File::new("..", FileAttributes::default()),
                    File::new(
                        "file",
                        FileAttributes {
                            size: Some(self.bytes.len() as u64),
                            permissions: Some(0o100_644),
                            ..Default::default()
                        },
                    ),
                    File::new(
                        "dir",
                        FileAttributes {
                            permissions: Some(0o040_755),
                            ..Default::default()
                        },
                    ),
                ],
            }))
        }
    }
    #[tokio::test]
    async fn protocol_roundtrip_directory_cache_expiry_and_upload_invalidation()
    -> std::result::Result<(), Box<dyn std::error::Error>> {
        tokio::time::timeout(Duration::from_secs(5), async {
            let (client, server) = tokio::io::duplex(8192);
            let requests = Arc::new(AtomicUsize::new(0));
            let server = tokio::spawn(russh_sftp::server::run(
                server,
                MemoryFs {
                    bytes: b"hello".to_vec(),
                    listed: false,
                    requests: requests.clone(),
                },
            ));
            let mut client = SftpClient {
                session: SftpSession::new(client).await?,
                cache: HashMap::new(),
            };
            let entries = client.list_dir(".").await?;
            assert_eq!(entries.len(), 2);
            assert_eq!(entries.first().map(|e| e.name.as_str()), Some("dir"));
            assert_eq!(client.list_dir(".").await?, entries);
            assert_eq!(requests.load(Ordering::Relaxed), 1);
            assert_eq!(client.read_file("file").await?, b"hello");
            client.write_file("file", b"updated UTF-8 \xd0\x96").await?;
            assert_eq!(client.read_file("file").await?, b"updated UTF-8 \xd0\x96");
            client.list_dir(".").await?;
            assert_eq!(requests.load(Ordering::Relaxed), 2);
            if let Some((time, _)) = client.cache.get_mut(".") {
                *time = Instant::now()
                    .checked_sub(Duration::from_secs(31))
                    .ok_or("clock range")?;
            }
            client.list_dir(".").await?;
            assert_eq!(requests.load(Ordering::Relaxed), 3);
            client.session.close().await?;
            server.await?;
            Ok::<_, Box<dyn std::error::Error>>(())
        })
        .await??;
        Ok(())
    }
}
