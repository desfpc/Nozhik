use super::*;
use russh::{
    Channel, ChannelId, Pty,
    keys::{
        PrivateKey,
        ssh_key::{Algorithm, HashAlg},
    },
    server,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
fn profile() -> SshProfile {
    SshProfile {
        name: "test".into(),
        host: "127.0.0.1".into(),
        port: 22,
        username: "user".into(),
        auth: AuthMethod::Password,
        host_key_sha256: format!("SHA256:{}", "A".repeat(43)),
        save_password: false,
        forwards: Vec::new(),
    }
}
#[test]
fn profiles_roundtrip_validation_and_corrupt_file_preservation() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config/ssh_hosts.json");
    assert_eq!(load_profiles(&path)?, [] as [SshProfile; 0]);
    let profiles = vec![
        profile(),
        SshProfile {
            name: "key".into(),
            auth: AuthMethod::PrivateKeyFile {
                path: "~/.ssh/id_ed25519".into(),
            },
            ..profile()
        },
        SshProfile {
            name: "agent".into(),
            auth: AuthMethod::Agent,
            ..profile()
        },
    ];
    save_profiles(&path, &profiles)?;
    assert_eq!(load_profiles(&path)?, profiles);
    let text = std::fs::read_to_string(&path)?;
    assert!(!text.contains("passphrase"));
    let without_port = serde_json::to_value(profile())?
        .as_object()
        .cloned()
        .ok_or("object expected")?
        .into_iter()
        .filter(|(key, _)| key != "port")
        .collect::<serde_json::Map<_, _>>();
    assert_eq!(
        serde_json::from_value::<SshProfile>(without_port.into())?.port,
        22
    );
    let mut unpinned = serde_json::to_value(profile())?;
    unpinned
        .as_object_mut()
        .ok_or("object expected")?
        .remove("host_key_sha256");
    let unpinned: SshProfile = serde_json::from_value(unpinned)?;
    unpinned.validate()?;
    assert_eq!(unpinned.host_key_sha256, "");
    let mut invalid = profile();
    invalid.port = 0;
    assert!(invalid.validate().is_err());
    invalid = profile();
    invalid.username.clear();
    assert!(invalid.validate().is_err());
    invalid = profile();
    invalid.host_key_sha256 = "123".into();
    assert!(invalid.validate().is_err());
    assert!(save_profiles(&path, &[profile(), profile()]).is_err());
    assert_eq!(load_profiles(&path)?, profiles);
    std::fs::write(&path, b"broken JSON")?;
    assert!(load_profiles(&path).is_err());
    assert_eq!(std::fs::read(&path)?, b"broken JSON");
    Ok(())
}
#[test]
fn sftp_attributes_preserve_unknowns_and_file_types() {
    use russh_sftp::protocol::FileAttributes;
    let attrs = FileAttributes {
        size: Some(123),
        permissions: Some(0o040_755),
        mtime: Some(100),
        ..Default::default()
    };
    let entry = sftp::RemoteFileEntry::from_attributes("dir".into(), &attrs);
    assert!(entry.is_dir);
    assert!(!entry.is_symlink);
    assert_eq!(entry.size, Some(123));
    assert_eq!(entry.modified, Some(100));
    let link = sftp::RemoteFileEntry::from_attributes(
        "link".into(),
        &FileAttributes {
            permissions: Some(0o120_777),
            ..Default::default()
        },
    );
    assert!(link.is_symlink);
    assert!(!link.is_dir);
    assert_eq!(link.size, None);
    let unknown = sftp::RemoteFileEntry::from_attributes("?".into(), &FileAttributes::default());
    assert_eq!(unknown.modified, None);
    assert_eq!(unknown.permissions, None);
}
struct Echo {
    rows: Arc<AtomicU32>,
}
impl server::Handler for Echo {
    type Error = russh::Error;
    fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> impl Future<Output = std::result::Result<server::Auth, Self::Error>> {
        std::future::ready(Ok(if user == "user" && password == "secret" {
            server::Auth::Accept
        } else {
            server::Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            }
        }))
    }
    async fn channel_open_session(
        &mut self,
        _channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> std::result::Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }
    fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        _cols: u32,
        rows: u32,
        _x: u32,
        _y: u32,
        _modes: &[(Pty, u32)],
        session: &mut server::Session,
    ) -> impl Future<Output = std::result::Result<(), Self::Error>> {
        assert_eq!(term, "xterm-256color");
        self.rows.store(rows, Ordering::Release);
        std::future::ready(session.channel_success(channel))
    }
    fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> impl Future<Output = std::result::Result<(), Self::Error>> {
        let result = session
            .data(channel, b"EARLY_BANNER".to_vec())
            .and_then(|()| session.channel_success(channel));
        std::future::ready(result)
    }
    fn window_change_request(
        &mut self,
        _channel: ChannelId,
        _cols: u32,
        rows: u32,
        _x: u32,
        _y: u32,
        _session: &mut server::Session,
    ) -> impl Future<Output = std::result::Result<(), Self::Error>> {
        self.rows.store(rows, Ordering::Release);
        std::future::ready(Ok(()))
    }
    fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> impl Future<Output = std::result::Result<(), Self::Error>> {
        std::future::ready(session.data(channel, data.to_vec()))
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        _channel: Channel<server::Msg>,
        host: &str,
        port: u32,
        _origin: &str,
        _origin_port: u32,
        reply: server::ChannelOpenHandle,
        _session: &mut server::Session,
    ) -> std::result::Result<(), Self::Error> {
        assert_eq!(host, "db.internal");
        assert_eq!(port, 5432);
        reply.accept().await;
        Ok(())
    }
}
#[tokio::test]
async fn real_handshake_pty_resize_tunnel_and_disconnect() -> TestResult {
    tokio::time::timeout(Duration::from_secs(10), async {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = Arc::new(server::Config {
            keys: vec![key],
            auth_rejection_time: Duration::ZERO,
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let rows = Arc::new(AtomicU32::new(0));
        let observed = rows.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            server::run_stream(config, socket, Echo { rows: observed })
                .await?
                .await
        });
        let profile = SshProfile {
            port,
            host_key_sha256: fingerprint,
            ..profile()
        };
        let ssh = SshSession::connect(&profile, Some("secret")).await?;
        let mut shell = ssh.open_shell(24, 80).await?;
        assert_eq!(rows.load(Ordering::Acquire), 24);
        assert!(matches!(shell.wait().await, Some(russh::ChannelMsg::Data { data }) if &data[..] == b"EARLY_BANNER"));
        shell.channel.window_change(100, 40, 0, 0).await?;
        shell.channel.data(&b"hello \x1b[31mSSH"[..]).await?;
        loop {
            if let Some(russh::ChannelMsg::Data { data }) = shell.channel.wait().await {
                assert_eq!(&data[..], b"hello \x1b[31mSSH");
                break;
            }
        }
        assert_eq!(rows.load(Ordering::Acquire), 40);
        let tunnel = ssh.start_tunnel(0, "db.internal", 5432).await?;
        assert!(tunnel.local_addr().ip().is_loopback());
        let mut stream = tokio::net::TcpStream::connect(tunnel.local_addr()).await?;
        stream.write_all(b"database bytes").await?;
        let mut bytes = [0; 14];
        stream.read_exact(&mut bytes).await?;
        assert_eq!(&bytes, b"database bytes");
        let address = tunnel.local_addr();
        tunnel.stop();
        tokio::task::yield_now().await;
        assert!(tokio::net::TcpStream::connect(address).await.is_err());
        ssh.disconnect().await?;
        assert!(ssh.open_shell(24, 80).await.is_err());
        match server.await? {
            Ok(()) => {}
            // Closing a client with open channels can reset TCP after SSH disconnect.
            Err(russh::Error::IO(e)) if e.kind() == std::io::ErrorKind::ConnectionReset => {}
            Err(e) => return Err(e.into()),
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}
#[tokio::test]
async fn wrong_host_key_and_stalled_handshake_fail_gracefully() -> TestResult {
    tokio::time::timeout(Duration::from_secs(20), async {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
        let config = Arc::new(server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let task = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            server::run_stream(
                config,
                socket,
                Echo {
                    rows: Arc::default(),
                },
            )
            .await?
            .await
        });
        assert!(
            SshSession::connect(&SshProfile { port, ..profile() }, Some("secret"))
                .await
                .is_err()
        );
        task.abort();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let task = tokio::spawn(async move {
            let (_socket, _) = listener.accept().await?;
            std::future::pending::<()>().await;
            Ok::<_, std::io::Error>(())
        });
        let result = SshSession::connect(&SshProfile { port, ..profile() }, Some("secret")).await;
        assert!(matches!(result, Err(Error::Timeout(_))));
        task.abort();
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn first_use_trust_is_explicit_persistent_and_precedes_password() -> TestResult {
    struct Audit(Arc<AtomicU32>);
    impl server::Handler for Audit {
        type Error = russh::Error;
        fn auth_password(
            &mut self,
            _user: &str,
            _password: &str,
        ) -> impl Future<Output = std::result::Result<server::Auth, Self::Error>> + Send {
            self.0.fetch_add(1, Ordering::Relaxed);
            std::future::ready(Ok(server::Auth::Accept))
        }
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = Arc::new(server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let count = Arc::new(AtomicU32::new(0));
        let observed = count.clone();
        let task = tokio::spawn(async move {
            let mut sessions = tokio::task::JoinSet::new();
            loop {
                let (socket, _) = listener.accept().await?;
                let config = config.clone();
                let observed = observed.clone();
                sessions.spawn(async move {
                    server::run_stream(config, socket, Audit(observed))
                        .await?
                        .await
                });
            }
            #[allow(
                unreachable_code,
                reason = "Accept loop is cancelled by the test after assertions"
            )]
            Ok::<_, std::io::Error>(())
        });
        let dir = tempfile::tempdir()?;
        let trust_path = dir.path().join("ssh_known_hosts.json");
        let profile = SshProfile {
            port,
            host_key_sha256: String::new(),
            save_password: false,
            ..profile()
        };
        profile.validate()?;
        let unresolved = trust::resolve(&trust_path, profile.clone())?;
        let error = SshSession::connect(&unresolved, Some("must not be sent"))
            .await
            .err()
            .ok_or("untrusted key was accepted")?;
        assert!(matches!(error,Error::UnknownHostKey(ref pin) if *pin == fingerprint));
        assert_eq!(count.load(Ordering::Relaxed), 0);
        assert!(!trust_path.exists()); // Cancelling the prompt never writes trust.
        trust::remember(&trust_path, &profile, &fingerprint)?;
        let trusted = trust::resolve(&trust_path, profile.clone())?;
        assert_eq!(trusted.host_key_sha256, fingerprint);
        SshSession::verify_host_key(&trusted).await?;
        assert_eq!(count.load(Ordering::Relaxed), 0);
        let session = SshSession::connect(&trusted, Some("secret")).await?;
        assert_eq!(count.load(Ordering::Relaxed), 1);
        session.disconnect().await?;
        let other = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?
            .public_key()
            .fingerprint(HashAlg::Sha256)
            .to_string();
        assert!(matches!(
            trust::remember(&trust_path, &profile, &other),
            Err(Error::HostKeyChanged { .. })
        ));
        let wrong = SshProfile {
            host_key_sha256: other,
            ..trusted
        };
        assert!(matches!(
            SshSession::verify_host_key(&wrong).await,
            Err(Error::HostKeyChanged { .. })
        ));
        assert_eq!(count.load(Ordering::Relaxed), 1);
        let text = std::fs::read_to_string(&trust_path)?;
        assert!(!text.contains("secret"));
        std::fs::write(&trust_path, "broken")?;
        assert!(trust::remember(&trust_path, &profile, &fingerprint).is_err());
        assert_eq!(std::fs::read_to_string(&trust_path)?, "broken");
        task.abort();
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}

#[cfg(unix)]
#[tokio::test]
#[ignore = "Requires /usr/sbin/sshd and an account allowed to launch an unprivileged local SSH server"]
async fn system_openssh_shell_and_sftp() -> TestResult {
    tokio::time::timeout(Duration::from_secs(15), async {
        use std::process::Stdio;
        let dir = tempfile::tempdir()?;
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
        let key_path = dir.path().join("key");
        key.write_openssh_file(&key_path, russh::keys::ssh_key::LineEnding::LF)?;
        let authorized = dir.path().join("authorized_keys");
        std::fs::write(&authorized, key.public_key().to_openssh()?)?;
        let username = tokio::process::Command::new("id").arg("-un").output().await?;
        if !username.status.success() { return Err("cannot identify test user".into()); }
        let username = String::from_utf8(username.stdout)?.trim().to_owned();
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port(); drop(listener);
        let config = dir.path().join("sshd_config");
        std::fs::write(&config, format!("Port {port}\nListenAddress 127.0.0.1\nHostKey {}\nAuthorizedKeysFile {}\nPidFile {}\nStrictModes no\nUsePAM no\nUseDNS no\nPasswordAuthentication no\nKbdInteractiveAuthentication no\nPubkeyAuthentication yes\nPermitRootLogin yes\nAllowUsers {username}\nSubsystem sftp internal-sftp\nLogLevel ERROR\n", key_path.display(), authorized.display(), dir.path().join("pid").display()))?;
        let log_path = dir.path().join("sshd.log");
        let mut daemon = tokio::process::Command::new("/usr/sbin/sshd").args(["-D","-e","-f"]).arg(&config)
            .stdin(Stdio::null()).stdout(Stdio::null()).stderr(std::fs::File::create(&log_path)?).kill_on_drop(true).spawn()?;
        loop {
            if let Some(status) = daemon.try_wait()? { return Err(format!("sshd {status}: {}",std::fs::read_to_string(&log_path)?).into()); }
            if tokio::net::TcpStream::connect(("127.0.0.1",port)).await.is_ok() { break; }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let profile = SshProfile { port, username, auth: AuthMethod::PrivateKeyFile { path: key_path }, host_key_sha256: key.public_key().fingerprint(HashAlg::Sha256).to_string(), ..profile() };
        let session = SshSession::connect(&profile,None).await?;
        let mut shell = session.open_shell(24,80).await?;
        shell.channel.data(&b"printf 'NOZHIK_%s\\n' 'OPENSSH_READY'\r"[..]).await?;
        let mut output = Vec::new();
        while !String::from_utf8_lossy(&output).contains("NOZHIK_OPENSSH_READY") {
            match shell.wait().await {
                Some(russh::ChannelMsg::Data { data }) => output.extend_from_slice(&data),
                Some(russh::ChannelMsg::Close) | None => return Err("OpenSSH shell closed before command output".into()),
                _ => {}
            }
        }
        let mut files = sftp::SftpClient::new(&session).await?;
        let path = dir.path().join("sftp_file").to_string_lossy().into_owned();
        files.write_file(&path,b"OpenSSH SFTP roundtrip").await?;
        assert_eq!(files.read_file(&path).await?,b"OpenSSH SFTP roundtrip");
        shell.channel.close().await?;
        session.disconnect().await?;
        daemon.kill().await?; daemon.wait().await?;
        Ok::<_,Box<dyn std::error::Error>>(())
    }).await??;
    Ok(())
}

#[tokio::test]
async fn actual_pty_and_shell_rejections_are_distinct() -> TestResult {
    struct Deny {
        pty: bool,
    }
    impl server::Handler for Deny {
        type Error = russh::Error;
        fn auth_password(
            &mut self,
            _user: &str,
            _password: &str,
        ) -> impl Future<Output = std::result::Result<server::Auth, Self::Error>> + Send {
            std::future::ready(Ok(server::Auth::Accept))
        }
        async fn channel_open_session(
            &mut self,
            _channel: Channel<server::Msg>,
            reply: server::ChannelOpenHandle,
            _session: &mut server::Session,
        ) -> std::result::Result<(), Self::Error> {
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
            _modes: &[(Pty, u32)],
            session: &mut server::Session,
        ) -> impl Future<Output = std::result::Result<(), Self::Error>> + Send {
            std::future::ready(if self.pty {
                session.channel_failure(channel)
            } else {
                session.channel_success(channel)
            })
        }
        fn shell_request(
            &mut self,
            channel: ChannelId,
            session: &mut server::Session,
        ) -> impl Future<Output = std::result::Result<(), Self::Error>> + Send {
            std::future::ready(session.channel_failure(channel))
        }
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        for pty in [true, false] {
            let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
            let pin = key.public_key().fingerprint(HashAlg::Sha256).to_string();
            let config = Arc::new(server::Config {
                keys: vec![key],
                ..Default::default()
            });
            let listener = TcpListener::bind("127.0.0.1:0").await?;
            let port = listener.local_addr()?.port();
            let task = tokio::spawn(async move {
                let (socket, _) = listener.accept().await?;
                server::run_stream(config, socket, Deny { pty })
                    .await?
                    .await
            });
            let session = SshSession::connect(
                &SshProfile {
                    port,
                    host_key_sha256: pin,
                    ..profile()
                },
                Some("secret"),
            )
            .await?;
            let error = session
                .open_shell(24, 80)
                .await
                .err()
                .ok_or("server denial was ignored")?;
            assert!(error.to_string().contains(if pty {
                "rejected PTY request"
            } else {
                "rejected shell request"
            }));
            task.abort();
        }
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}
