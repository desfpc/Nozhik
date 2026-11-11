use super::*;
use ::redis::AsyncCommands as _;
use std::{
    process::{Child, Command, Stdio},
    time::{Duration, Instant},
};

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
fn text(value: &str) -> Atom {
    Atom::Text(value.into())
}
#[test]
fn profiles_and_editor_are_lossless_and_preserve_bad_files() -> TestResult {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("redis.json");
    let profile = Profile {
        name: "local".into(),
        ..Profile::default()
    };
    save_profiles(&path, std::slice::from_ref(&profile))?;
    assert_eq!(load_profiles(&path)?, vec![profile.clone()]);
    assert!(!std::fs::read_to_string(&path)?.contains("\"password\":"));
    assert!(save_profiles(&path, &[profile.clone(), profile]).is_err());
    std::fs::write(&path, "{broken")?;
    assert!(save_profiles(&path, &[]).is_err());
    assert_eq!(std::fs::read_to_string(&path)?, "{broken");
    let bytes = vec![0, 255, 13, 10];
    assert_eq!(Atom::from_bytes(bytes.clone()).bytes()?, bytes);
    assert!(Atom::Hex { hex: "é".into() }.bytes().is_err());
    assert!(Data::parse(r#"{"type":"zset","value":[["a","NaN"]]}"#).is_err());
    assert!(Data::parse(r#"{"type":"set","value":["a","a"]}"#).is_err());
    assert!(Data::parse(r#"{"type":"hash","value":[["a","1"],["a","2"]]}"#).is_err());
    assert!(Data::parse(r#"{"type":"list","value":[]}"#).is_err());
    assert_eq!(
        transport::substring_pattern("a*[?]\\"),
        "*a\\*\\[\\?\\]\\\\*"
    );
    Ok(())
}
struct Server {
    child: Child,
    _directory: tempfile::TempDir,
    profile: Profile,
}
impl Drop for Server {
    fn drop(&mut self) {
        if self.child.kill().is_ok() {
            let _status = self.child.wait();
        }
    }
}
impl Server {
    async fn start() -> std::result::Result<Self, Box<dyn std::error::Error>> {
        let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        let directory = tempfile::tempdir()?;
        let executable =
            std::env::var_os("NOZHIK_REDIS_SERVER").unwrap_or_else(|| "redis-server".into());
        let child = Command::new(executable)
            .args([
                "--bind",
                "127.0.0.1",
                "--port",
                &port.to_string(),
                "--save",
                "",
                "--appendonly",
                "no",
                "--databases",
                "32",
                "--requirepass",
                "redis-test-only",
            ])
            .current_dir(directory.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        let server = Self {
            child,
            _directory: directory,
            profile: Profile {
                name: "test".into(),
                host: "127.0.0.1".into(),
                port,
                ..Profile::default()
            },
        };
        let deadline = Instant::now() + Duration::from_secs(5);
        while Connection::connect(&server.profile, Some(&Self::password()), None)
            .await
            .is_err()
        {
            if Instant::now() >= deadline {
                return Err("Redis test server did not become ready".into());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok(server)
    }
    fn password() -> Secret {
        Secret::new("redis-test-only".into())
    }
    async fn connection(&self) -> Result<Connection> {
        Connection::connect(&self.profile, Some(&Self::password()), None).await
    }
    async fn raw(&self) -> ::redis::RedisResult<::redis::aio::MultiplexedConnection> {
        ::redis::Client::open(::redis::ConnectionInfo {
            addr: ::redis::ConnectionAddr::Tcp(self.profile.host.clone(), self.profile.port),
            redis: ::redis::RedisConnectionInfo {
                password: Some("redis-test-only".into()),
                ..Default::default()
            },
        })?
        .get_multiplexed_async_connection()
        .await
    }
}
#[tokio::test]
#[ignore = "requires redis-server; override executable with NOZHIK_REDIS_SERVER"]
async fn structures_conflicts_expiry_binary_and_database_isolation() -> TestResult {
    let server = Server::start().await?;
    let db = server.connection().await?;
    assert!(
        Connection::connect(&server.profile, Some(&Secret::new("wrong".into())), None)
            .await
            .is_err()
    );
    let cases = [
        (
            "string",
            Data::String(Atom::from_bytes(vec![0, 255])),
            Data::String(text("updated")),
        ),
        (
            "hash",
            Data::Hash(vec![(text("a"), text("1"))]),
            Data::Hash(vec![(text("b"), text("2")), (text("c"), text("3"))]),
        ),
        (
            "list",
            Data::List(vec![text("one"), text("two")]),
            Data::List(vec![text("two"), text("three")]),
        ),
        (
            "set",
            Data::Set(vec![text("one")]),
            Data::Set(vec![text("three"), text("two")]),
        ),
        (
            "zset",
            Data::Zset(vec![(text("one"), "1".into())]),
            Data::Zset(vec![(text("two"), "2.5".into())]),
        ),
    ];
    for (name, before, after) in cases {
        let key = text(name);
        db.create(0, &key, &before).await?;
        assert!(db.create(0, &key, &before).await.is_err());
        let snapshot = db.read(0, &key).await?;
        assert_eq!(snapshot.data, before);
        db.mutate(0, &snapshot, &Mutation::Expire(Some(60))).await?;
        db.mutate(0, &snapshot, &Mutation::Replace(after.clone()))
            .await?;
        let fresh = db.read(0, &key).await?;
        assert_eq!(fresh.data, after);
        assert!(fresh.ttl_ms > 55_000 && fresh.ttl_ms <= 60_000);
        assert!(db.mutate(0, &snapshot, &Mutation::Delete).await.is_err());
        db.mutate(0, &fresh, &Mutation::Expire(None)).await?;
        assert_eq!(db.read(0, &key).await?.ttl_ms, -1);
        db.mutate(0, &fresh, &Mutation::Delete).await?;
        assert!(db.read(0, &key).await.is_err());
    }
    db.create(17, &text("only:17"), &Data::String(text("separate")))
        .await?;
    assert!(db.read(0, &text("only:17")).await.is_err());
    assert_eq!(
        db.read(17, &text("only:17")).await?.data,
        Data::String(text("separate"))
    );
    assert!(db.scan(32, 0, "").await.is_err());
    let mut raw = server.raw().await?;
    let _: () = raw.set_ex("expires", "soon", 1).await?;
    let stale = db.read(0, &text("expires")).await?;
    tokio::time::sleep(Duration::from_millis(1100)).await;
    assert!(
        db.mutate(
            0,
            &stale,
            &Mutation::Replace(Data::String(text("resurrect")))
        )
        .await
        .is_err()
    );
    assert!(db.read(0, &text("expires")).await.is_err());
    Ok(())
}
#[tokio::test]
#[ignore = "requires redis-server; override executable with NOZHIK_REDIS_SERVER"]
async fn scan_literal_streams_and_partial_collections() -> TestResult {
    let server = Server::start().await?;
    let db = server.connection().await?;
    let mut raw = server.raw().await?;
    for key in ["prefix:a*[?]:tail", "prefix:abc:tail", "unrelated"] {
        let _: () = raw.set(key, "value").await?;
    }
    let mut cursor = 0;
    let mut found = Vec::new();
    loop {
        let page = db.scan(0, cursor, "a*[?]").await?;
        found.extend(page.rows);
        cursor = page.cursor;
        if cursor == 0 {
            break;
        }
    }
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].key, text("prefix:a*[?]:tail"));
    assert_eq!(found[0].preview, "value");
    assert_eq!(found[0].ttl_ms, -1);
    let _: String = ::redis::cmd("XADD")
        .arg("events")
        .arg("*")
        .arg("field")
        .arg("first")
        .query_async(&mut raw)
        .await?;
    let _: () = ::redis::cmd("XGROUP")
        .arg("CREATE")
        .arg("events")
        .arg("workers")
        .arg("0")
        .query_async(&mut raw)
        .await?;
    let snapshot = db.read(0, &text("events")).await?;
    db.mutate(
        0,
        &snapshot,
        &Mutation::Append(vec![(text("field"), text("second"))]),
    )
    .await?;
    let fresh = db.read(0, &text("events")).await?;
    let Data::Stream(records) = &fresh.data else {
        return Err("Expected stream".into());
    };
    assert_eq!(records.len(), 2);
    let id = records.first().ok_or("missing stream record")?.0.clone();
    db.mutate(0, &fresh, &Mutation::DeleteEntry(id)).await?;
    let Data::Stream(records) = db.read(0, &text("events")).await?.data else {
        return Err("Expected stream".into());
    };
    assert_eq!(records.len(), 1);
    let groups: ::redis::Value = ::redis::cmd("XINFO")
        .arg("GROUPS")
        .arg("events")
        .query_async(&mut raw)
        .await?;
    assert!(matches!(groups, ::redis::Value::Array(ref items) if items.len() == 1));
    let many: Vec<_> = (0..1100).map(|n| n.to_string()).collect();
    let _: usize = raw.rpush("large", many).await?;
    let large = db.read(0, &text("large")).await?;
    assert!(large.truncated);
    assert!(db.mutate(0, &large, &Mutation::Delete).await.is_err());
    let bytes = vec![b'x'; transport::MAX_BYTES + 1];
    let _: () = raw.set("big-string", bytes).await?;
    let oversized = db.read(0, &text("big-string")).await;
    assert!(oversized.is_err() || oversized.is_ok_and(|s| s.truncated));
    Ok(())
}
#[tokio::test]
async fn worker_preserves_profiles_and_rejects_unverified_credentials() -> TestResult {
    use worker::{Action, Output, Request, Worker};
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("redis.json");
    let profile = Profile {
        name: "saved".into(),
        ..Profile::default()
    };
    let mut worker = Worker::start(path.clone(), std::sync::Arc::new(|| {}));
    let token = crate::CancellationToken::new();
    assert!(worker.send(Request {
        id: 1,
        cancel: token.clone(),
        action: Action::Save {
            original: None,
            profile: profile.clone(),
            password: Some(Secret::new("session-only".into()))
        }
    }));
    let reply = tokio::time::timeout(Duration::from_secs(3), worker.output.recv())
        .await?
        .ok_or("reply")?;
    assert_eq!(reply.id, 1);
    assert!(matches!(reply.result?, Output::Profiles(p) if p == vec![profile.clone()]));
    assert!(!std::fs::read_to_string(&path)?.contains("session-only"));
    let mut protected = profile.clone();
    protected.save_password = true;
    assert!(worker.send(Request {
        id: 2,
        cancel: token,
        action: Action::Save {
            original: Some(profile.clone()),
            profile: protected,
            password: Some(Secret::new("not-verified".into()))
        }
    }));
    assert!(
        tokio::time::timeout(Duration::from_secs(3), worker.output.recv())
            .await?
            .ok_or("reply")?
            .result
            .is_err()
    );
    assert_eq!(load_profiles(&path)?, vec![profile]);
    worker.shutdown();
    Ok(())
}

#[tokio::test]
#[ignore = "requires redis-server; override executable with NOZHIK_REDIS_SERVER"]
async fn redis_through_verified_ssh_and_tunnel_shutdown() -> TestResult {
    tokio::time::timeout(Duration::from_secs(20), async {
        use russh::keys::{
            PrivateKey,
            ssh_key::{Algorithm, HashAlg},
        };
        let redis = Server::start().await?;
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = std::sync::Arc::new(russh::server::Config {
            keys: vec![key],
            ..Default::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            russh::server::run_stream(config, socket, crate::tests::ForwardDatabase)
                .await?
                .await
        });
        let ssh_profile = nozhik_remote_ssh::SshProfile {
            name: "redis-test".into(),
            host: "127.0.0.1".into(),
            port,
            username: "test".into(),
            auth: nozhik_remote_ssh::AuthMethod::Password,
            host_key_sha256: fingerprint,
            save_password: false,
            forwards: Vec::new(),
        };
        let ssh =
            nozhik_remote_ssh::SshSession::connect(&ssh_profile, Some("ssh-test-only")).await?;
        let profile = Profile {
            host: "db.internal".into(),
            ssh_profile_id: Some(ssh_profile.name),
            ..redis.profile.clone()
        };
        assert!(
            Connection::connect(&profile, Some(&Server::password()), None)
                .await
                .is_err()
        );
        let db = Connection::connect(&profile, Some(&Server::password()), Some(&ssh)).await?;
        db.create(17, &text("tunneled"), &Data::String(text("through ssh")))
            .await?;
        assert_eq!(
            db.read(17, &text("tunneled")).await?.data,
            Data::String(text("through ssh"))
        );
        let page = db.scan(17, 0, "tunneled").await?;
        assert_eq!(page.rows.len(), 1);
        ssh.disconnect().await?;
        assert!(db.read(17, &text("tunneled")).await.is_err());
        drop(db);
        server.abort();
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
#[ignore = "requires redis-server; override executable with NOZHIK_REDIS_SERVER"]
async fn denied_replacement_keeps_original_key_and_expiry() -> TestResult {
    let server = Server::start().await?;
    let db = server.connection().await?;
    let key = text("guarded");
    db.create(0, &key, &Data::List(vec![text("original")]))
        .await?;
    let original = db.read(0, &key).await?;
    db.mutate(0, &original, &Mutation::Expire(Some(60))).await?;
    let mut raw = server.raw().await?;
    let _: () = ::redis::cmd("ACL")
        .arg("SETUSER")
        .arg("limited")
        .arg("on")
        .arg(">limited-only")
        .arg("~*")
        .arg("+@all")
        .arg("-rename")
        .query_async(&mut raw)
        .await?;
    let limited = Connection::connect(
        &Profile {
            user: "limited".into(),
            ..server.profile.clone()
        },
        Some(&Secret::new("limited-only".into())),
        None,
    )
    .await?;
    assert!(
        limited
            .mutate(
                0,
                &original,
                &Mutation::Replace(Data::List(vec![text("replacement")]))
            )
            .await
            .is_err()
    );
    let unchanged = db.read(0, &key).await?;
    assert_eq!(unchanged.data, original.data);
    assert!(unchanged.ttl_ms > 55_000);
    let temporary: Vec<String> = raw.keys("*nozhik*").await?;
    assert_eq!(temporary, Vec::<String>::new());
    Ok(())
}

#[tokio::test]
#[ignore = "requires Redis 7.4+ server; override executable with NOZHIK_REDIS_SERVER"]
async fn hash_field_expiry_survives_structured_replacement() -> TestResult {
    let server = Server::start().await?;
    let db = server.connection().await?;
    let key = text("field-ttl");
    db.create(
        0,
        &key,
        &Data::Hash(vec![
            (text("expiring"), text("old")),
            (text("persistent"), text("kept")),
        ]),
    )
    .await?;
    let mut raw = server.raw().await?;
    let _: Vec<i64> = ::redis::cmd("HPEXPIRE")
        .arg("field-ttl")
        .arg(60_000)
        .arg("FIELDS")
        .arg(1)
        .arg("expiring")
        .query_async(&mut raw)
        .await?;
    let snapshot = db.read(0, &key).await?;
    db.mutate(
        0,
        &snapshot,
        &Mutation::Replace(Data::Hash(vec![
            (text("expiring"), text("new")),
            (text("persistent"), text("kept")),
            (text("added"), text("fresh")),
        ])),
    )
    .await?;
    let ttl: Vec<i64> = ::redis::cmd("HPTTL")
        .arg("field-ttl")
        .arg("FIELDS")
        .arg(3)
        .arg("expiring")
        .arg("persistent")
        .arg("added")
        .query_async(&mut raw)
        .await?;
    assert!(ttl.first().is_some_and(|n| *n > 55_000 && *n <= 60_000));
    assert_eq!(ttl.get(1), Some(&-1));
    assert_eq!(ttl.get(2), Some(&-1));
    Ok(())
}
