use super::*;
use crate::{
    Connection, DbDriverType,
    schema::{ExportOptions, Format, Output, Task},
};

#[test]
fn dynamic_results_preserve_types_empty_columns_and_limits() -> Result<()> {
    let mut decoder = Decoder::default();
    let mut rows = 0;
    let mut bytes = 0;
    decoder.line(
        br#"["id","id","price","array","nullable","score"]"#,
        &mut rows,
        &mut bytes,
    )?;
    decoder.line(
        br#"["UInt64","UInt128","Decimal(38,9)","Array(UInt64)","Nullable(String)","Float64"]"#,
        &mut rows,
        &mut bytes,
    )?;
    assert_eq!(decoder.result.columns.len(), 6);
    assert_eq!(decoder.result.rows.len(), 0);
    decoder.line(br#"["18446744073709551615","340282366920938463463374607431768211455","12345678901234567890.123456789",["1","2"],null,"1.25"]"#, &mut rows, &mut bytes)?;
    assert_eq!(
        decoder.result.rows[0][0],
        DbValue::Text(u64::MAX.to_string())
    );
    assert_eq!(
        decoder.result.rows[0][2],
        DbValue::Text("12345678901234567890.123456789".into())
    );
    assert_eq!(
        decoder.result.rows[0][3],
        DbValue::Json(r#"["1","2"]"#.into())
    );
    assert_eq!(decoder.result.rows[0][4], DbValue::Null);
    assert_eq!(decoder.result.rows[0][5], DbValue::Float(1.25));
    rows = 10_000;
    assert!(!decoder.line(br#"["1","2","0",[],null,"0"]"#, &mut rows, &mut bytes)?);
    assert!(decoder.result.truncated);
    assert_eq!(decoder.result.rows.len(), 1);
    assert!(
        decoder
            .line(b"Code: 123. late server failure", &mut 0, &mut 0)
            .is_err()
    );
    Ok(())
}

#[test]
fn scripts_quotes_and_backend_readonly_gate() -> Result<()> {
    let sql = "-- ignored ;\n SELECT 'a;\\\'b'; /* ; */ SELECT `a;b`, 'c'';d'; # ;\n SELECT 3";
    assert_eq!(statements(sql)?.len(), 3);
    assert!(statements("SELECT 'unterminated").is_err());
    assert!(statements("SELECT 1 /* unterminated").is_err());
    assert_eq!(
        statements("SELECT $tag$a;\'b$tag$; SELECT $$x;y$$")?.len(),
        2
    );
    assert_eq!(
        crate::connection::quote_identifier(DbDriverType::ClickHouse, "a`b\\c"),
        "`a\\`b\\\\c`"
    );
    let changes = crate::changes::TableChanges::new(
        DbDriverType::ClickHouse,
        "default".into(),
        "t".into(),
        QueryResult::default(),
        vec![],
    );
    assert!(changes.statements().is_err());
    let old: ConnectionProfile = serde_json::from_str(
        r#"{"name":"old","driver":"Sqlite","host":"localhost","port":5432,"user":"","database":":memory:","ssh_profile_id":null}"#,
    )?;
    assert!(!old.tls);
    Ok(())
}

#[tokio::test]
#[ignore = "Requires an explicitly isolated ClickHouse server via NOZHIK_TEST_CLICKHOUSE_PORT"]
async fn system_clickhouse_query_catalog_export_truncate_and_cancel()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(90), async {
        let profile = ConnectionProfile { name: "clickhouse isolated fixture".into(), driver: DbDriverType::ClickHouse, host: "127.0.0.1".into(), port: std::env::var("NOZHIK_TEST_CLICKHOUSE_PORT")?.parse()?, user: "nozhik".into(), database: "nozhik".into(), ..ConnectionProfile::default() };
        let password = Secret::new("nozhik-test-only".into());
        let db = Connection::connect(&profile, Some(&password), None).await?;
        let cancel = CancellationToken::new();
        db.query("DROP TABLE IF EXISTS nozhik.phase_clickhouse; CREATE TABLE nozhik.phase_clickhouse (id UInt64, big UInt128, price Decimal(38,9), label Nullable(String), items Array(UInt64), pair Tuple(String,UInt8), score Float64, at DateTime, computed UInt64 MATERIALIZED id+1) ENGINE=MergeTree PARTITION BY id%2 ORDER BY id TTL at + INTERVAL 10 YEAR; INSERT INTO nozhik.phase_clickhouse (id,big,price,label,items,pair,score,at) VALUES (1,340282366920938463463374607431768211455,12345678901234567890.123456789,NULL,[1,2],('hello',2),1.25,'2026-11-11 00:00:00')", None, &cancel).await?;
        let results = db.query("SELECT * FROM phase_clickhouse; SELECT id AS duplicate,id AS duplicate FROM phase_clickhouse WHERE 0", Some("nozhik"), &cancel).await?;
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].rows[0][1], DbValue::Text("340282366920938463463374607431768211455".into()));
        assert_eq!(results[0].rows[0][2], DbValue::Text("12345678901234567890.123456789".into()));
        assert_eq!(results[0].rows[0][3], DbValue::Null);
        assert_eq!(results[0].rows[0][6], DbValue::Float(1.25));
        assert_eq!(results[1].columns.len(), 2);
        assert_eq!(results[1].rows.len(), 0);
        let info = db.describe_table("nozhik", "phase_clickhouse", false).await?;
        assert!(info.columns[0].is_primary_key);
        assert!(info.objects.error.is_none(), "{:?}", info.objects.error);
        assert!(info.objects.ddl.contains("TTL"));
        assert!(info.objects.keys.iter().any(|k| k.name == "PARTITION BY"));
        assert!(db.catalog().await?.schemas.iter().any(|s| s.name == "nozhik" && s.tables.iter().any(|t| t.name == "phase_clickhouse")));
        db.query("CREATE VIEW IF NOT EXISTS nozhik.phase_clickhouse_view AS SELECT id FROM nozhik.phase_clickhouse", None, &cancel).await?;
        assert!(db.catalog().await?.schemas.iter().any(|s| s.name == "nozhik" && s.tables.iter().any(|t| t.name == "phase_clickhouse_view" && t.is_view)));
        assert!(db.query("SELECT nonexistent_column FROM phase_clickhouse", Some("nozhik"), &cancel).await.is_err());
        assert!(db.query("USE default", None, &cancel).await.is_err());
        let options = ExportOptions { only_table: true, table: "phase_clickhouse".into(), ..ExportOptions::default() };
        let Output::Artifact(artifact) = db.schema_task("nozhik", Task::Export(options.clone()), &cancel).await? else { return Err("Missing SQL export".into()); };
        let Output::Clipboard(dump) = db.schema_task("nozhik", Task::Clipboard(artifact), &cancel).await? else { return Err("Missing clipboard SQL".into()); };
        assert!(dump.contains("CREATE TABLE"));
        assert!(dump.contains("340282366920938463463374607431768211455"));
        db.schema_task("nozhik", Task::Truncate("phase_clickhouse".into()), &cancel).await?;
        let count = db.query("SELECT count() FROM phase_clickhouse", Some("nozhik"), &cancel).await?;
        assert_eq!(count[0].rows[0][0], DbValue::Int(0));
        // Restore the exported values; computed columns must be omitted from INSERT.
        let inserts = statements(&dump)?.into_iter().filter(|s| s.starts_with("INSERT")).collect::<Vec<_>>().join(";");
        db.query(&inserts, Some("nozhik"), &cancel).await?;
        let restored = db.query("SELECT * FROM phase_clickhouse", Some("nozhik"), &cancel).await?;
        assert_eq!(restored[0].rows, results[0].rows);
        db.query("INSERT INTO phase_clickhouse (id,big,price,label,items,pair,score,at) SELECT number+2,number,0,'bulk',[],('',0),0,now() FROM numbers(10001)", Some("nozhik"), &cancel).await?;
        let limited = db.query("SELECT id FROM phase_clickhouse", Some("nozhik"), &cancel).await?;
        assert!(limited[0].truncated);
        assert_eq!(limited[0].rows.len(), 10_000);
        let options = ExportOptions { format: Format::Csv, generated: false, ..options };
        let Output::Artifact(artifact) = db.schema_task("nozhik", Task::Export(options), &cancel).await? else { return Err("Missing CSV export".into()); };
        let Output::Clipboard(csv) = db.schema_task("nozhik", Task::Clipboard(artifact), &cancel).await? else { return Err("Missing clipboard CSV".into()); };
        assert_eq!(csv.lines().count(), 10003);
        assert!(!csv.lines().next().unwrap_or_default().contains("computed"));
        let script = tempfile::NamedTempFile::new()?;
        std::fs::write(script.path(), "SELECT number FROM numbers(10002); CREATE TABLE nozhik.phase_clickhouse_script(id UInt64) ENGINE=Memory;")?;
        db.schema_task("nozhik", Task::RunFile(script.path().to_owned()), &cancel).await?;
        assert!(db.describe_table("nozhik", "phase_clickhouse_script", false).await.is_ok());
        db.query("DROP TABLE nozhik.phase_clickhouse_script", None, &cancel).await?;
        let stop = CancellationToken::new();
        let operation = db.query("SELECT sleep(3)", None, &stop);
        let cancel_later = async { tokio::time::sleep(Duration::from_millis(100)).await; stop.cancel(); };
        let (cancelled, ()) = tokio::join!(operation, cancel_later);
        assert!(matches!(cancelled, Err(Error::Cancelled)));
        let bad = Secret::new("incorrect-test-password".into());
        assert!(Connection::connect(&profile, Some(&bad), None).await.is_err());
        let map = db.query("SELECT map('a',1,'a',2)", None, &cancel).await?;
        assert_eq!(map[0].rows[0][0].to_string().matches("\"a\"").count(), 2);
        db.query("CREATE TABLE nozhik.phase_clickhouse_binary(value String) ENGINE=Memory; INSERT INTO nozhik.phase_clickhouse_binary VALUES (unhex('00FF80D09FD180'))", None, &cancel).await?;
        assert!(db.query("SELECT value FROM nozhik.phase_clickhouse_binary", None, &cancel).await.is_err());
        let Output::Artifact(artifact) = db.schema_task("nozhik", Task::Export(ExportOptions { only_table: true, table: "phase_clickhouse_binary".into(), ddl: false, ..ExportOptions::default() }), &cancel).await? else { return Err("Missing binary SQL export".into()); };
        let Output::Clipboard(binary_sql) = db.schema_task("nozhik", Task::Clipboard(artifact), &cancel).await? else { return Err("Missing binary SQL".into()); };
        assert!(binary_sql.is_ascii());
        db.schema_task("nozhik", Task::Truncate("phase_clickhouse_binary".into()), &cancel).await?;
        db.query(&binary_sql, None, &cancel).await?;
        let binary = db.query("SELECT hex(value) FROM nozhik.phase_clickhouse_binary", None, &cancel).await?;
        assert_eq!(binary[0].rows[0][0], DbValue::Text("00FF80D09FD180".into()));
        db.query("DROP VIEW nozhik.phase_clickhouse_view; DROP TABLE nozhik.phase_clickhouse; DROP TABLE nozhik.phase_clickhouse_binary", None, &cancel).await?;
        db.close().await;
        Ok::<_, Box<dyn std::error::Error>>(())
    }).await??;
    Ok(())
}

#[tokio::test]
#[ignore = "Requires an explicitly isolated ClickHouse server via NOZHIK_TEST_CLICKHOUSE_PORT"]
async fn system_clickhouse_verified_ssh() -> std::result::Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(20), async {
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
            russh::server::run_stream(config, socket, crate::tests::ForwardDatabase)
                .await?
                .await
        });
        let ssh_profile = nozhik_remote_ssh::SshProfile {
            name: "clickhouse SSH fixture".into(),
            host: "127.0.0.1".into(),
            port,
            username: "test".into(),
            auth: nozhik_remote_ssh::AuthMethod::Password,
            host_key_sha256: fingerprint,
            save_password: false,
            forwards: vec![],
        };
        let ssh =
            nozhik_remote_ssh::SshSession::connect(&ssh_profile, Some("ssh-test-only")).await?;
        let profile = ConnectionProfile {
            name: "tunneled ClickHouse".into(),
            driver: DbDriverType::ClickHouse,
            host: "db.internal".into(),
            port: std::env::var("NOZHIK_TEST_CLICKHOUSE_PORT")?.parse()?,
            user: "nozhik".into(),
            database: "nozhik".into(),
            ssh_profile_id: Some(ssh_profile.name),
            ..ConnectionProfile::default()
        };
        let db = Connection::connect(
            &profile,
            Some(&Secret::new("nozhik-test-only".into())),
            Some(&ssh),
        )
        .await?;
        let result = db
            .query("SELECT 77", None, &CancellationToken::new())
            .await?;
        assert_eq!(result[0].rows[0][0], DbValue::Int(77));
        assert!(db.healthy().await);
        db.close().await;
        ssh.disconnect().await?;
        server.abort();
        Ok::<_, Box<dyn std::error::Error>>(())
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn http_fragmented_rows_and_late_errors_are_not_reported_as_success()
-> std::result::Result<(), Box<dyn std::error::Error>> {
    tokio::time::timeout(Duration::from_secs(10), async {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await?;
        let port = listener.local_addr()?.port();
        let server = tokio::spawn(async move {
            for (status, body) in [("200 OK", "[\"1\"]\n[\"UInt8\"]\n[1]\n"), ("200 OK", "[\"x\",\"x\"]\n[\"String\",\"UInt8\"]\n[\"Привет\\nworld\",2]\n"), ("200 OK", "[\"x\"]\n[\"UInt8\"]\n[1]\nCode: 999. Late execution failure\n"), ("401 Unauthorized", "Code: 516. Authentication failed\n")] {
                let (mut socket, _) = listener.accept().await?;
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") {
                    socket.read_exact(&mut byte).await?;
                    request.push(byte[0]);
                }
                let request = String::from_utf8(request)?;
                assert!(request.contains("authorization: Basic "));
                assert!(!request.contains("password="));
                let size: usize = request.lines().find_map(|l| l.strip_prefix("content-length: ")).ok_or("missing content-length")?.parse()?;
                socket.read_exact(&mut vec![0; size]).await?;
                socket.write_all(format!("HTTP/1.1 {status}\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n").as_bytes()).await?;
                for chunk in body.as_bytes().chunks(3) {
                    socket.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await?;
                    socket.write_all(chunk).await?;
                    socket.write_all(b"\r\n").await?;
                }
                socket.write_all(b"0\r\n\r\n").await?;
            }
            Ok::<_, Box<dyn std::error::Error + Send + Sync>>(())
        });
        let profile = ConnectionProfile { name: "HTTP fixture".into(), driver: DbDriverType::ClickHouse, host: "127.0.0.1".into(), port, user: "default".into(), database: "default".into(), ..ConnectionProfile::default() };
        let db = Connection::connect(&profile, Some(&Secret::new("private".into())), None).await?;
        let cancel = CancellationToken::new();
        let results = db.query("SELECT 'Привет\\nworld', 2", None, &cancel).await?;
        assert_eq!(results[0].columns.len(), 2);
        assert_eq!(results[0].rows[0][0], DbValue::Text("Привет\nworld".into()));
        assert!(db.query("SELECT 1", None, &cancel).await.is_err());
        assert!(db.query("SELECT 1", None, &cancel).await.is_err());
        server.await?.map_err(|e| e.to_string())?;
        Ok::<_, Box<dyn std::error::Error>>(())
    }).await??;
    Ok(())
}
