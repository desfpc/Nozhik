use crate::*;
use nozhik_remote_ssh::credentials::Secret;

#[test]
fn profiles_roundtrip_validation_and_malformed_preservation()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("config/database_connections.json");
    let mut profiles = Vec::new();
    for driver in [
        DbDriverType::Postgres,
        DbDriverType::Mysql,
        DbDriverType::Sqlite,
    ] {
        profiles.push(ConnectionProfile {
            name: format!("{driver:?}"),
            driver,
            user: "user".into(),
            save_password: true,
            ..ConnectionProfile::default()
        });
    }
    save_profiles(&path, &profiles)?;
    assert_eq!(load_profiles(&path)?, profiles);
    let json = std::fs::read_to_string(&path)?;
    assert!(!json.contains("secret-password"));
    save_profiles(&path, &profiles)?;
    let mut duplicate = profiles.clone();
    duplicate.push(profiles[0].clone());
    assert!(save_profiles(&path, &duplicate).is_err());
    assert_eq!(load_profiles(&path)?, profiles);
    std::fs::write(&path, b"{broken")?;
    assert!(save_profiles(&path, &profiles).is_err());
    assert_eq!(std::fs::read(&path)?, b"{broken");
    assert!(
        ConnectionProfile {
            name: "bad".into(),
            ssh_profile_id: Some("ssh".into()),
            ..ConnectionProfile::default()
        }
        .validate()
        .is_err()
    );
    assert_eq!(
        connection::preview_sql(DbDriverType::Sqlite, "main", "odd\"; DROP TABLE t;--"),
        "SELECT * FROM \"main\".\"odd\"\"; DROP TABLE t;--\" LIMIT 1000"
    );
    Ok(())
}
async fn memory() -> Result<Connection> {
    Connection::connect(
        &ConnectionProfile {
            name: "test".into(),
            ..ConnectionProfile::default()
        },
        Some(&Secret::new("not-serialized".into())),
        None,
    )
    .await
}
#[tokio::test]
async fn sqlite_typed_queries_scripts_and_metadata()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let db = memory().await?;
    let cancel = CancellationToken::new();
    db.query("CREATE TABLE people (id INTEGER PRIMARY KEY, name TEXT NOT NULL, score REAL, payload BLOB, missing TEXT, active BOOLEAN, created DATETIME, doc JSON); CREATE VIEW names AS SELECT name FROM people;",None,&cancel).await?;
    let inserted=db.query("INSERT INTO people VALUES (7,'Привет',2.5,x'00ff',NULL,TRUE,'2026-10-26 12:00:00','{\"ok\":true}');",None,&cancel).await?;
    assert_eq!(inserted.first().ok_or("missing insert")?.rows_affected, 1);
    let results = db
        .query("SELECT * FROM people; SELECT 42 AS answer;", None, &cancel)
        .await?;
    assert_eq!(results.len(), 2);
    let result = results.first().ok_or("missing SELECT")?;
    assert_eq!(result.columns.len(), 8);
    let row = result.rows.first().ok_or("missing row")?;
    assert_eq!(row.first(), Some(&DbValue::Int(7)));
    assert_eq!(row.get(1), Some(&DbValue::Text("Привет".into())));
    assert_eq!(row.get(2), Some(&DbValue::Float(2.5)));
    assert_eq!(row.get(3), Some(&DbValue::Bytes(vec![0, 255])));
    assert_eq!(row.get(4), Some(&DbValue::Null));
    assert_eq!(row.get(5), Some(&DbValue::Bool(true)));
    assert_eq!(
        row.get(6),
        Some(&DbValue::DateTime("2026-10-26 12:00:00".into()))
    );
    // SQLite JSON has TEXT storage; typed Json is supplied by PostgreSQL/MySQL.
    assert_eq!(row.get(7), Some(&DbValue::Text("{\"ok\":true}".into())));
    let empty = db
        .query("SELECT name FROM people WHERE 0", None, &cancel)
        .await?;
    assert_eq!(
        empty
            .first()
            .ok_or("missing empty SELECT")?
            .columns
            .first()
            .ok_or("missing column")?
            .name,
        "name"
    );
    assert!(
        db.query("SELECT nonexistent FROM people", None, &cancel)
            .await
            .is_err()
    );
    assert_eq!(
        db.query("SELECT id FROM people", None, &cancel)
            .await?
            .first()
            .map(|r| r.rows.len()),
        Some(1)
    );
    let tree = db.introspect().await?;
    let schema = tree
        .schemas
        .iter()
        .find(|s| s.name == "main")
        .ok_or("missing schema")?;
    let table = schema
        .tables
        .iter()
        .find(|t| t.name == "people")
        .ok_or("missing table")?;
    assert_eq!(table.columns.len(), 8);
    assert!(table.columns.first().ok_or("missing id")?.is_primary_key);
    assert!(!table.columns.get(1).ok_or("missing name")?.nullable);
    assert!(schema.tables.iter().any(|t| t.name == "names" && t.is_view));
    Ok(())
}
#[tokio::test]
async fn cancellation_and_result_limits() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let db = memory().await?;
    let token = CancellationToken::new();
    token.cancel();
    assert!(matches!(
        db.query("SELECT 1", None, &token).await,
        Err(Error::Cancelled)
    ));
    let token = CancellationToken::new();
    let trigger = token.clone();
    let cancel = async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        trigger.cancel();
    };
    let query=db.query("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<1000000000) SELECT sum(x) FROM n",None,&token);
    let (result, ()) = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        tokio::join!(query, cancel)
    })
    .await?;
    assert!(matches!(result, Err(Error::Cancelled)));
    let results=db.query("WITH RECURSIVE n(x) AS (VALUES(0) UNION ALL SELECT x+1 FROM n WHERE x<20000) SELECT x FROM n",None,&CancellationToken::new()).await?;
    let result = results.first().ok_or("missing recursive result")?;
    assert!(result.truncated);
    assert_eq!(result.rows.len(), 10000);
    Ok(())
}
#[tokio::test]
async fn sqlite_file_reconnect_preserves_data()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let dir = tempfile::tempdir()?;
    let profile = ConnectionProfile {
        name: "file".into(),
        database: dir
            .path()
            .join("test.sqlite")
            .to_string_lossy()
            .into_owned(),
        ..ConnectionProfile::default()
    };
    let db = Connection::connect(&profile, None, None).await?;
    db.query(
        "CREATE TABLE saved(id INTEGER); INSERT INTO saved VALUES(5)",
        None,
        &CancellationToken::new(),
    )
    .await?;
    drop(db);
    let db = Connection::connect(&profile, None, None).await?;
    let results = db
        .query("SELECT id FROM saved", None, &CancellationToken::new())
        .await?;
    assert_eq!(
        results
            .first()
            .and_then(|r| r.rows.first())
            .and_then(|r| r.first()),
        Some(&DbValue::Int(5))
    );
    Ok(())
}

#[tokio::test]
async fn background_worker_correlates_queries_and_rejects_untested_saved_password()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use crate::worker::{Command, Reply, Worker};
    let dir = tempfile::tempdir()?;
    let mut worker = Worker::start(
        dir.path().join("connections.json"),
        std::sync::Arc::new(|| {}),
    );
    let profile = ConnectionProfile {
        name: "background".into(),
        ..ConnectionProfile::default()
    };
    assert!(worker.send(Command::Connect {
        id: 1,
        profile: profile.clone(),
        password: None,
        ssh: None,
        test: false
    }));
    let connected = tokio::time::timeout(std::time::Duration::from_secs(5), worker.output.recv())
        .await?
        .ok_or("worker closed")?;
    assert!(matches!(connected, Reply::Connected(1, _, false, Ok(_))));
    assert!(worker.send(Command::Query {
        id: 2,
        name: profile.name.clone(),
        sql: "SELECT 123 AS value".into(),
        schema: None,
        cancel: CancellationToken::new()
    }));
    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), worker.output.recv())
        .await?
        .ok_or("worker closed")?;
    assert!(
        matches!(reply,Reply::Query(2,Ok(results)) if results.first().and_then(|r|r.rows.first()).and_then(|r|r.first())==Some(&DbValue::Int(123)))
    );
    assert!(worker.send(Command::Save(
        ConnectionProfile {
            save_password: true,
            ..profile
        },
        Some(Secret::new("never-store-untested".into()))
    )));
    let reply = tokio::time::timeout(std::time::Duration::from_secs(5), worker.output.recv())
        .await?
        .ok_or("worker closed")?;
    assert!(matches!(reply,Reply::SaveError(message) if message.contains("Test this connection")));
    Ok(())
}

#[tokio::test]
#[ignore = "Requires temporary localhost PostgreSQL and MySQL; ports supplied through NOZHIK_TEST_PG_PORT and NOZHIK_TEST_MYSQL_PORT"]
async fn system_postgres_mysql_types_schema_and_cancellation()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(std::time::Duration::from_secs(30),async {
        for driver in [DbDriverType::Postgres,DbDriverType::Mysql] {
            let port=std::env::var(if driver==DbDriverType::Postgres {"NOZHIK_TEST_PG_PORT"}else{"NOZHIK_TEST_MYSQL_PORT"})?.parse()?;
            let profile=ConnectionProfile {name:"system-test".into(),driver,host:"127.0.0.1".into(),port,user:if driver==DbDriverType::Postgres {"nozhik"}else{"root"}.into(),database:"nozhik".into(),..ConnectionProfile::default()};
            let password=Secret::new("nozhik-test-only".into());let db=Connection::connect(&profile,Some(&password),None).await?;
            let cancel=CancellationToken::new();
            let setup=if driver==DbDriverType::Postgres {"DROP TABLE IF EXISTS phase8; CREATE TABLE phase8(id BIGINT PRIMARY KEY, name TEXT NOT NULL, doc JSON, created TIMESTAMP, active BOOLEAN); INSERT INTO phase8 VALUES(7,'тест','{\"ok\":true}','2026-10-26 12:00:00',TRUE)"}else{"DROP TABLE IF EXISTS phase8; CREATE TABLE phase8(id BIGINT PRIMARY KEY, name TEXT NOT NULL, doc JSON, created DATETIME, active BOOLEAN); INSERT INTO phase8 VALUES(7,'тест','{\"ok\":true}','2026-10-26 12:00:00',TRUE)"};
            db.query(setup,None,&cancel).await?;
            let sql=if driver==DbDriverType::Postgres {"SELECT id,name,doc,created,active,NULL AS missing,12.25::numeric AS decimal,2.5::float8 AS real FROM phase8"}else{"SELECT id,name,doc,created,active,NULL AS missing,CAST(12.25 AS DECIMAL(20,2)) AS exact_value,CAST(18446744073709551615 AS UNSIGNED) AS unsigned_value FROM phase8"};
            let results=db.query(sql,None,&cancel).await?;let row=results.first().and_then(|r|r.rows.first()).ok_or("missing row")?;
            assert_eq!(row.first(),Some(&DbValue::Int(7)));assert_eq!(row.get(1),Some(&DbValue::Text("тест".into())));
            assert!(matches!(row.get(3),Some(DbValue::DateTime(_))));assert_eq!(row.get(5),Some(&DbValue::Null));assert_eq!(row.get(6),Some(&DbValue::Text("12.25".into())));
            if driver==DbDriverType::Postgres {assert!(matches!(row.get(2),Some(DbValue::Json(_))));assert_eq!(row.get(4),Some(&DbValue::Bool(true)));}
            else {assert_eq!(row.get(7),Some(&DbValue::Text("18446744073709551615".into())));}
            let tree=db.introspect().await?;let table=tree.schemas.iter().flat_map(|s|&s.tables).find(|t|t.name=="phase8").ok_or("missing table")?;
            assert!(table.columns.first().ok_or("missing PK")?.is_primary_key);assert!(!table.columns.get(1).ok_or("missing name")?.nullable);
            let token=CancellationToken::new();let trigger=token.clone();
            let sql=if driver==DbDriverType::Postgres {"SELECT pg_sleep(10)"}else{"SELECT SLEEP(10)"};
            let (result,())=tokio::join!(db.query(sql,None,&token),async move{tokio::time::sleep(std::time::Duration::from_millis(80)).await;trigger.cancel();});
            assert!(matches!(result,Err(Error::Cancelled)));
        }
        Ok::<_,Box<dyn std::error::Error>>(())
    }).await??;
    Ok(())
}

struct ForwardDatabase;
impl russh::server::Handler for ForwardDatabase {
    type Error = russh::Error;
    fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> impl Future<Output = std::result::Result<russh::server::Auth, Self::Error>> {
        std::future::ready(Ok(if user == "test" && password == "ssh-test-only" {
            russh::server::Auth::Accept
        } else {
            russh::server::Auth::Reject {
                proceed_with_methods: None,
                partial_success: false,
            }
        }))
    }
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: russh::Channel<russh::server::Msg>,
        host: &str,
        port: u32,
        _origin: &str,
        _origin_port: u32,
        reply: russh::server::ChannelOpenHandle,
        _session: &mut russh::server::Session,
    ) -> std::result::Result<(), Self::Error> {
        assert_eq!(host, "db.internal");
        let port = u16::try_from(port).map_err(|e| std::io::Error::other(e.to_string()))?;
        let mut socket = tokio::net::TcpStream::connect(("127.0.0.1", port)).await?;
        reply.accept().await;
        tokio::spawn(async move {
            let mut stream = channel.into_stream();
            if let Err(e) = tokio::io::copy_bidirectional(&mut socket, &mut stream).await {
                eprintln!("Test forwarding shutdown: {e}");
            }
        });
        Ok(())
    }
}
#[tokio::test]
#[ignore = "Requires temporary localhost PostgreSQL on NOZHIK_TEST_PG_PORT"]
async fn system_postgres_through_authenticated_ssh_tunnel()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        use russh::keys::{
            PrivateKey,
            ssh_key::{Algorithm, HashAlg},
        };
        let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)?;
        let fingerprint = key.public_key().fingerprint(HashAlg::Sha256).to_string();
        let config = std::sync::Arc::new(russh::server::Config {
            keys: vec![key],
            ..russh::server::Config::default()
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await?;
            russh::server::run_stream(config, socket, ForwardDatabase)
                .await?
                .await
        });
        let ssh_profile = nozhik_remote_ssh::SshProfile {
            name: "db-test".into(),
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
        let db_profile = ConnectionProfile {
            name: "tunneled".into(),
            driver: DbDriverType::Postgres,
            host: "db.internal".into(),
            port: std::env::var("NOZHIK_TEST_PG_PORT")?.parse()?,
            user: "nozhik".into(),
            database: "nozhik".into(),
            ssh_profile_id: Some(ssh_profile.name),
            save_password: false,
        };
        let db = Connection::connect(
            &db_profile,
            Some(&Secret::new("nozhik-test-only".into())),
            Some(&ssh),
        )
        .await?;
        let results = db
            .query("SELECT 77 AS tunneled", None, &CancellationToken::new())
            .await?;
        assert_eq!(
            results
                .first()
                .and_then(|r| r.rows.first())
                .and_then(|r| r.first()),
            Some(&DbValue::Int(77))
        );
        assert!(!db.introspect().await?.schemas.is_empty());
        drop(db);
        ssh.disconnect().await?;
        server.abort();
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}

async fn edit_snapshot(
    db: &Connection,
    table: &str,
    keys: Vec<usize>,
) -> Result<changes::TableChanges> {
    let result = db
        .query(
            &connection::preview_sql(DbDriverType::Sqlite, "main", table),
            None,
            &CancellationToken::new(),
        )
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| Error::Invalid("Missing result".into()))?;
    Ok(changes::TableChanges::new(
        DbDriverType::Sqlite,
        "main".into(),
        table.into(),
        result,
        keys,
    ))
}
#[tokio::test]
async fn table_edits_stage_preview_submit_revert_and_objects()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let db = memory().await?;
    let token = CancellationToken::new();
    db.query("CREATE TABLE parent(id INTEGER PRIMARY KEY); CREATE TABLE items(id INTEGER PRIMARY KEY, name TEXT NOT NULL DEFAULT 'default', parent_id INTEGER REFERENCES parent(id), payload BLOB); CREATE INDEX items_name ON items(name); INSERT INTO items(id,name) VALUES(1,'old'),(2,'delete');",None,&token).await?;
    let tree = db.introspect().await?;
    let table = tree.schemas[0]
        .tables
        .iter()
        .find(|t| t.name == "items")
        .ok_or("items")?;
    assert!(!table.objects.keys.is_empty());
    assert_eq!(table.objects.foreign_keys.len(), 1);
    assert_eq!(table.objects.indexes.len(), 1);
    assert!(table.objects.ddl.contains("CREATE TABLE items"));
    assert!(table.objects.ddl.contains("CREATE INDEX items_name"));
    let mut edits = edit_snapshot(&db, "items", vec![0]).await?;
    let text = "quote ' \\ ; DROP TABLE items; -- Привет\n\0";
    edits.rows[0].values[0] = Some(DbValue::Int(3));
    edits.rows[0].values[1] = Some(DbValue::Text(text.into()));
    edits.rows[1].deleted = true;
    edits.add_row();
    edits.rows[2].values[3] = Some(DbValue::Bytes(vec![0, 255]));
    assert!(edits.is_dirty());
    let sql = edits.statements()?.join("\n");
    assert!(sql.contains("UPDATE"));
    assert!(sql.contains("DELETE"));
    assert!(sql.contains("INSERT"));
    assert!(sql.contains("WHERE \"id\" = 1"));
    let unchanged = edit_snapshot(&db, "items", vec![0]).await?;
    assert_eq!(unchanged.baseline.rows[0][1], DbValue::Text("old".into()));
    db.apply_changes(&edits, &token).await?;
    let fresh = edit_snapshot(&db, "items", vec![0]).await?;
    assert_eq!(fresh.rows.len(), 2);
    assert_eq!(fresh.baseline.rows[0][0], DbValue::Int(3));
    assert_eq!(fresh.baseline.rows[0][1], DbValue::Text(text.into()));
    assert_eq!(fresh.baseline.rows[1][1], DbValue::Text("default".into()));
    assert_eq!(fresh.baseline.rows[1][3], DbValue::Bytes(vec![0, 255]));
    edits.revert();
    assert!(!edits.is_dirty());
    assert_eq!(edits.statements()?, Vec::<String>::new());
    Ok(())
}
#[tokio::test]
async fn submit_rolls_back_constraint_stale_and_ambiguous_rows()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let db = memory().await?;
    let token = CancellationToken::new();
    db.query("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT NOT NULL); INSERT INTO items VALUES(1,'one'),(2,'two'); CREATE TABLE duplicates(value TEXT); INSERT INTO duplicates VALUES('same'),('same');",None,&token).await?;
    let mut edits = edit_snapshot(&db, "items", vec![0]).await?;
    edits.rows[0].values[1] = Some(DbValue::Text("changed".into()));
    edits.rows[1].values[1] = Some(DbValue::Null);
    assert!(db.apply_changes(&edits, &token).await.is_err());
    assert!(edits.is_dirty());
    assert_eq!(
        edit_snapshot(&db, "items", vec![0]).await?.baseline.rows[0][1],
        DbValue::Text("one".into())
    );
    edits.rows[1].values[1] = Some(DbValue::Text("second".into()));
    db.query(
        "UPDATE items SET name='concurrent' WHERE id=2",
        None,
        &token,
    )
    .await?;
    assert!(db.apply_changes(&edits, &token).await.is_err());
    assert_eq!(
        edit_snapshot(&db, "items", vec![0]).await?.baseline.rows[0][1],
        DbValue::Text("one".into())
    );
    let mut duplicates = edit_snapshot(&db, "duplicates", vec![]).await?;
    duplicates.rows[0].deleted = true;
    assert!(db.apply_changes(&duplicates, &token).await.is_err());
    assert_eq!(
        edit_snapshot(&db, "duplicates", vec![]).await?.rows.len(),
        2
    );
    db.query("INSERT INTO duplicates VALUES('unique')", None, &token)
        .await?;
    let mut unique = edit_snapshot(&db, "duplicates", vec![]).await?;
    unique.rows[2].values[0] = Some(DbValue::Text("updated".into()));
    db.apply_changes(&unique, &token).await?;
    Ok(())
}
#[tokio::test]
async fn submit_cancel_interrupts_sqlite_trigger_and_rolls_back()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    let db = memory().await?;
    let token = CancellationToken::new();
    db.query("CREATE TABLE items(id INTEGER PRIMARY KEY,name TEXT); INSERT INTO items VALUES(1,'old'); CREATE TABLE slow(value INTEGER); CREATE TRIGGER slow_update AFTER UPDATE ON items BEGIN INSERT INTO slow SELECT * FROM (WITH RECURSIVE n(x) AS(SELECT 1 UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT x FROM n); END;",None,&token).await?;
    let mut edits = edit_snapshot(&db, "items", vec![0]).await?;
    edits.rows[0].values[1] = Some(DbValue::Text("new".into()));
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let (result, ()) = tokio::join!(db.apply_changes(&edits, &cancel), async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        trigger.cancel();
    });
    assert!(result.is_err());
    let fresh = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        edit_snapshot(&db, "items", vec![0]),
    )
    .await??;
    assert_eq!(fresh.baseline.rows[0][1], DbValue::Text("old".into()));
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert!(db.apply_changes(&edits, &cancelled).await.is_err());
    Ok(())
}

#[tokio::test]
async fn catalog_connection_is_lazy_and_metadata_failure_is_local()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use worker::{Command, Reply, Worker};
    let dir = tempfile::tempdir()?;
    let profile = ConnectionProfile {
        name: "lazy".into(),
        database: dir.path().join("lazy.sqlite").display().to_string(),
        ..Default::default()
    };
    let db = Connection::connect(&profile, None, None).await?;
    db.query("CREATE TABLE good(id INTEGER PRIMARY KEY); CREATE VIEW broken AS SELECT * FROM missing_table;",None,&CancellationToken::new()).await?;
    let catalog = db.catalog().await?;
    assert_eq!(catalog.schemas[0].tables.len(), 2);
    assert!(
        catalog.schemas[0]
            .tables
            .iter()
            .all(|t| !t.details_loaded && t.columns.is_empty() && t.objects.ddl.is_empty())
    );
    let good = db.describe_table("main", "good", false).await?;
    assert!(good.details_loaded);
    assert_eq!(good.columns.len(), 1);
    assert!(db.describe_table("main", "broken", true).await.is_err());
    let mut worker = Worker::start(
        dir.path().join("connections.json"),
        std::sync::Arc::new(|| {}),
    );
    assert!(worker.send(Command::Connect {
        id: 100,
        profile: profile.clone(),
        password: None,
        ssh: None,
        test: false
    }));
    let reply = tokio::time::timeout(std::time::Duration::from_secs(3), worker.output.recv())
        .await?
        .ok_or("reply")?;
    assert!(
        matches!(reply,Reply::Connected(100,_,false,Ok(tree)) if tree.schemas[0].tables.len()==2)
    );
    assert!(worker.send(Command::Describe {
        id: 101,
        name: profile.name.clone(),
        schema: "main".into(),
        table: "broken".into(),
        is_view: true
    }));
    let reply = tokio::time::timeout(std::time::Duration::from_secs(3), worker.output.recv())
        .await?
        .ok_or("reply")?;
    assert!(matches!(reply, Reply::Table(101, Err(_))));
    assert!(worker.send(Command::Describe {
        id: 102,
        name: profile.name,
        schema: "main".into(),
        table: "good".into(),
        is_view: false
    }));
    let reply = tokio::time::timeout(std::time::Duration::from_secs(3), worker.output.recv())
        .await?
        .ok_or("reply")?;
    assert!(matches!(reply,Reply::Table(102,Ok(t)) if t.details_loaded && t.columns.len()==1));
    worker.shutdown();
    db.close().await;
    Ok(())
}
#[tokio::test]
async fn editing_profile_replaces_atomically_and_disconnects_old_pool()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    use worker::{Command, Reply, Worker};
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("connections.json");
    let original = ConnectionProfile {
        name: "original".into(),
        ..Default::default()
    };
    let other = ConnectionProfile {
        name: "other".into(),
        ..Default::default()
    };
    save_profiles(&path, &[original.clone(), other.clone()])?;
    let mut worker = Worker::start(path.clone(), std::sync::Arc::new(|| {}));
    assert!(worker.send(Command::Connect {
        id: 1,
        profile: original.clone(),
        password: None,
        ssh: None,
        test: false
    }));
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(3), worker.output.recv()).await?,
        Some(Reply::Connected(1, _, false, Ok(_)))
    ));
    let mut edited = original.clone();
    edited.name = "renamed".into();
    edited.database = dir.path().join("replacement.sqlite").display().to_string();
    assert!(worker.send(Command::Update {
        original: original.clone(),
        profile: edited.clone(),
        password: None
    }));
    assert!(
        matches!(tokio::time::timeout(std::time::Duration::from_secs(3),worker.output.recv()).await?,Some(Reply::Updated(name,profiles)) if name=="original" && profiles==vec![edited.clone(),other.clone()])
    );
    assert_eq!(load_profiles(&path)?, vec![edited.clone(), other]);
    assert!(worker.send(Command::Query {
        id: 2,
        name: "original".into(),
        sql: "SELECT 1".into(),
        schema: None,
        cancel: CancellationToken::new()
    }));
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(3), worker.output.recv()).await?,
        Some(Reply::Query(2, Err(_)))
    ));
    let persisted = std::fs::read(&path)?;
    assert!(worker.send(Command::Update {
        original: original.clone(),
        profile: edited.clone(),
        password: None
    }));
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(3), worker.output.recv()).await?,
        Some(Reply::SaveError(_))
    ));
    assert_eq!(std::fs::read(&path)?, persisted);
    let mut duplicate = edited.clone();
    duplicate.name = "other".into();
    assert!(worker.send(Command::Update {
        original: edited.clone(),
        profile: duplicate,
        password: None
    }));
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(3), worker.output.recv()).await?,
        Some(Reply::SaveError(_))
    ));
    assert_eq!(std::fs::read(&path)?, persisted);
    let mut protected = edited.clone();
    protected.save_password = true;
    assert!(worker.send(Command::Update {
        original: edited,
        profile: protected,
        password: Some(Secret::new("not-tested".into()))
    }));
    assert!(matches!(
        tokio::time::timeout(std::time::Duration::from_secs(3), worker.output.recv()).await?,
        Some(Reply::SaveError(_))
    ));
    assert_eq!(std::fs::read(&path)?, persisted);
    worker.shutdown();
    Ok(())
}
