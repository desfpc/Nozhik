use super::*;
use crate::ConnectionProfile;
use std::time::Duration;

type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
async fn artifact(db: &Connection, schema: &str, task: Task) -> Result<Arc<Artifact>> {
    match db
        .schema_task(schema, task, &CancellationToken::new())
        .await?
    {
        Output::Artifact(file) => Ok(file),
        _ => Err(Error::Invalid("Expected artifact".into())),
    }
}
#[test]
fn dialect_modification_quoting_grants_and_delimiters() -> TestResult {
    let old = Settings {
        name: "db`name".into(),
        collation: "utf8mb4_general_ci".into(),
        collations: vec!["utf8mb4_general_ci".into(), "utf8mb4_bin".into()],
        ..Settings::default()
    };
    let mut new = old.clone();
    new.collation = "utf8mb4_bin".into();
    new.grants.push(Grant {
        user: "user`name".into(),
        host: "localhost".into(),
        privilege: "ALTER".into(),
    });
    let sql = modification_sql(DbDriverType::Mysql, &old, &new)?;
    assert!(sql.contains("ALTER DATABASE `db``name` COLLATE `utf8mb4_bin`;"));
    assert!(sql.contains("TO `user``name`@`localhost`"));
    new.name = "renamed".into();
    assert!(modification_sql(DbDriverType::Mysql, &old, &new).is_err());
    new.name.clone_from(&old.name);
    new.grants[0].privilege = "ALTER; DROP DATABASE x".into();
    assert!(modification_sql(DbDriverType::Mysql, &old, &new).is_err());
    let old = Settings {
        name: "old\"name".into(),
        owner: "role".into(),
        ..Settings::default()
    };
    let mut new = old.clone();
    new.name = "new name".into();
    new.owner = "other\"role".into();
    new.comment = "'quoted'\\value".into();
    new.grants.push(Grant {
        user: "PUBLIC".into(),
        host: String::new(),
        privilege: "USAGE".into(),
    });
    let sql = modification_sql(DbDriverType::Postgres, &old, &new)?;
    assert!(sql.contains("TO PUBLIC;"));
    assert!(sql.contains("RENAME TO \"new name\";"));
    assert!(sql.contains("OWNER TO \"other\"\"role\";"));
    let batches = script_batches(
        "SELECT 'line\nDELIMITER bad\nvalue';\n/*\nDELIMITER bad\n*/\nDELIMITER $$\nCREATE PROCEDURE p() BEGIN SELECT '$$'; SELECT 2; END$$\nDELIMITER ;\nSELECT 3;",
        DbDriverType::Mysql,
    )?;
    assert_eq!(batches.len(), 3);
    assert!(batches[0].contains("DELIMITER bad"));
    assert!(batches[1].contains("SELECT '$$'; SELECT 2; END"));
    assert_eq!(batches[2].trim(), "SELECT 3;");
    assert_eq!(
        csv_line(&["a,b".into(), "a\"b\nc".into(), String::new()], ','),
        "\"a,b\",\"a\"\"b\nc\",\r\n"
    );
    Ok(())
}
#[tokio::test]
async fn sqlite_schema_export_restore_csv_and_script_beyond_console_limit() -> TestResult {
    tokio::time::timeout(Duration::from_secs(30),async {
        let dir=tempfile::tempdir()?;let profile=ConnectionProfile {name:"schema-test".into(),database:dir.path().join("original.sqlite").display().to_string(),..ConnectionProfile::default()};let db=Connection::connect(&profile,None,None).await?;let token=CancellationToken::new();
        db.query("CREATE TABLE items(id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT, amount INTEGER, doubled INTEGER GENERATED ALWAYS AS (amount*2) STORED); CREATE TABLE audit(value TEXT); CREATE INDEX item_names ON items(name); CREATE VIEW item_view AS SELECT name FROM items; CREATE TRIGGER item_audit AFTER INSERT ON items BEGIN INSERT INTO audit VALUES(new.name); END; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<10005) INSERT INTO items(name,amount) SELECT 'a,\"b'||char(10)||x,x FROM n; UPDATE sqlite_sequence SET seq=20000 WHERE name='items';",None,&token).await?;
        let Output::Settings(old)=db.schema_task("main",Task::Settings,&token).await? else {return Err("settings".into());};let mut new=old.clone();new.user_version=21;new.application_id=42;
        assert!(modification_sql(DbDriverType::Sqlite,&old,&new)?.contains("user_version = 21"));
        db.schema_task("main",Task::Modify(Box::new(old.clone()),Box::new(new.clone())),&token).await?;assert!(db.schema_task("main",Task::Modify(Box::new(old),Box::new(new)),&token).await.is_err());
        let script=dir.path().join("script.sql");tokio::fs::write(&script,"SELECT * FROM items; INSERT INTO audit VALUES('after 10005 rows');").await?;db.schema_task("main",Task::RunFile(script.clone()),&token).await?;
        tokio::fs::write(&script,"BEGIN; INSERT INTO audit VALUES('rolled back'); SELECT missing FROM nowhere;").await?;assert!(db.schema_task("main",Task::RunFile(script.clone()),&token).await.is_err());
        let dump=artifact(&db,"main",Task::Export(ExportOptions::default())).await?;assert!(dump.bytes>64*1024);assert!(dump.preview.contains("Preview limited"));let saved=dir.path().join("dump.sql");db.schema_task("main",Task::Save(dump.clone(),saved.clone()),&token).await?;assert_eq!(tokio::fs::metadata(&saved).await?.len(),dump.bytes);
        let ddl=artifact(&db,"main",Task::Ddl).await?;let text=tokio::fs::read_to_string(ddl.file.path()).await?;assert!(text.contains("CREATE TRIGGER"));assert!(!text.contains("INSERT INTO \"items\""));
        let restored=Connection::connect(&ConnectionProfile {name:"restore".into(),database:dir.path().join("restored.sqlite").display().to_string(),..ConnectionProfile::default()},None,None).await?;restored.schema_task("main",Task::RunFile(saved),&token).await?;
        let result=restored.query("SELECT count(*),max(doubled) FROM items; SELECT count(*) FROM audit; PRAGMA user_version; INSERT INTO items(name,amount) VALUES('last',1); SELECT max(id) FROM items;",None,&token).await?;
        assert_eq!(result[0].rows[0],vec![DbValue::Int(10005),DbValue::Int(20010)]);assert_eq!(result[1].rows[0][0],DbValue::Int(10006));assert_eq!(result[2].rows[0][0],DbValue::Int(21));assert_eq!(result[4].rows[0][0],DbValue::Int(20001));
        let options=ExportOptions {format:Format::Csv,table:"items".into(),generated:false,row_header:true,null:"\\N".into(),..ExportOptions::default()};let csv=artifact(&db,"main",Task::Export(options)).await?;let text=tokio::fs::read_to_string(csv.file.path()).await?;assert!(text.starts_with("#,id,name,amount\r\n"));assert!(text.contains("10005,10005,"));assert!(!text.contains("doubled"));assert!(text.contains("\"a,\"\"b\n1\""));
        tokio::fs::write(&script,"CREATE TABLE small(a TEXT,b TEXT); INSERT INTO small VALUES('one',NULL),('two','value');").await?;db.schema_task("main",Task::RunFile(script),&token).await?;
        let csv=artifact(&db,"main",Task::Export(ExportOptions {format:Format::Csv,table:"small".into(),transpose:true,null:"\\N".into(),..ExportOptions::default()})).await?;assert_eq!(tokio::fs::read_to_string(csv.file.path()).await?,"a,one,two\r\nb,\\N,value\r\n");
        let stopped=CancellationToken::new();stopped.cancel();assert!(matches!(db.schema_task("main",Task::Ddl,&stopped).await,Err(Error::Cancelled)));
        db.close().await;restored.close().await;Ok::<_,Box<dyn std::error::Error>>(())
    }).await??;
    Ok(())
}

/// Uses only dedicated throwaway servers; no local client programs are invoked.
#[tokio::test]
#[ignore = "Requires disposable PostgreSQL/MariaDB on NOZHIK_SCHEMA_PG_PORT/NOZHIK_SCHEMA_MYSQL_PORT"]
async fn system_schema_catalog_dump_restore_without_external_tools() -> TestResult {
    tokio::time::timeout(Duration::from_secs(90),async {
        for driver in [DbDriverType::Postgres,DbDriverType::Mysql] {
            let port=std::env::var(if driver==DbDriverType::Postgres{"NOZHIK_SCHEMA_PG_PORT"}else{"NOZHIK_SCHEMA_MYSQL_PORT"})?.parse()?;
            let db=Connection::connect(&ConnectionProfile {name:"export-fixture".into(),driver,host:"127.0.0.1".into(),port,user:if driver==DbDriverType::Postgres{"postgres"}else{"root"}.into(),database:"nozhik".into(),..ConnectionProfile::default()},Some(&nozhik_remote_ssh::credentials::Secret::new("nozhik-fixture-only".into())),None).await?;
            let schema=if driver==DbDriverType::Postgres{"export \"schema"}else{"export `schema"};let q=|s:&str|quote_identifier(driver,s);let token=CancellationToken::new();
            let setup=if driver==DbDriverType::Postgres {format!("DROP SCHEMA IF EXISTS {} CASCADE; CREATE SCHEMA {}; SET search_path TO {}; CREATE TYPE mood AS ENUM ('a','b'); CREATE DOMAIN positive AS integer CHECK(VALUE>0); CREATE TABLE z_parent(id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,name TEXT, exact numeric(30,5), bytes bytea, tags text[], feeling mood, n positive); CREATE TABLE a_child(id BIGINT PRIMARY KEY,parent_id BIGINT REFERENCES z_parent(id), doubled BIGINT GENERATED ALWAYS AS (id*2) STORED); CREATE INDEX child_parent ON a_child(parent_id); CREATE FUNCTION plus_one(i integer) RETURNS integer LANGUAGE SQL IMMUTABLE AS $$ SELECT i+1 $$; CREATE TABLE audit(id bigint); CREATE FUNCTION audit_row() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO audit VALUES(NEW.id); RETURN NEW; END $$; CREATE TRIGGER log_row AFTER INSERT ON z_parent FOR EACH ROW EXECUTE FUNCTION audit_row(); INSERT INTO z_parent(name,exact,bytes,tags,feeling,n) SELECT 'a,''\\line'||chr(10)||i,12345678901234567890.12345,decode('00ff','hex'),ARRAY['a,b','c'], 'a',1 FROM generate_series(1,10005)i; INSERT INTO a_child VALUES(1,1); CREATE VIEW z_view AS SELECT name FROM z_parent; CREATE VIEW a_view AS SELECT name FROM z_view; CREATE MATERIALIZED VIEW cached AS SELECT count(*) AS n FROM z_parent; COMMENT ON TABLE z_parent IS 'table comment'; COMMENT ON COLUMN z_parent.name IS 'column comment'; ALTER TABLE z_parent ENABLE ROW LEVEL SECURITY; CREATE POLICY everyone ON z_parent FOR SELECT USING(true);",q(schema),q(schema),q(schema))}else{format!("DROP DATABASE IF EXISTS {}; CREATE DATABASE {} CHARACTER SET utf8mb4; USE {}; CREATE TABLE z_parent(id BIGINT UNSIGNED AUTO_INCREMENT PRIMARY KEY,name TEXT,exact DECIMAL(30,5),bytes BLOB,created DATETIME(6),flags BIT(8),location POINT); CREATE TABLE a_child(id BIGINT PRIMARY KEY,parent_id BIGINT UNSIGNED, doubled BIGINT GENERATED ALWAYS AS (id*2) STORED, FOREIGN KEY(parent_id) REFERENCES z_parent(id)); CREATE TABLE audit(id BIGINT); CREATE TRIGGER log_row AFTER INSERT ON z_parent FOR EACH ROW INSERT INTO audit VALUES(NEW.id); INSERT INTO z_parent(name,exact,bytes,created,flags,location) VALUES(CONCAT('a,',CHAR(39),'\\\\line',CHAR(10)),12345678901234567890.12345,X'00ff','2026-11-02 12:34:56.123456',b'10000001',ST_GeomFromText('POINT(1 2)',4326)); INSERT INTO a_child(id,parent_id) VALUES(1,1); CREATE VIEW z_view AS SELECT name FROM z_parent; CREATE VIEW a_view AS SELECT name FROM z_view;",q(schema),q(schema),q(schema))};
            db.execute_schema_script(&setup,if driver==DbDriverType::Postgres{"public"}else{"nozhik"},&token).await?;
            if driver==DbDriverType::Mysql {db.execute_schema_script("DELIMITER $$\nCREATE PROCEDURE do_math() BEGIN SELECT 1; SELECT 2; END$$\nDELIMITER ;\nCREATE EVENT future_event ON SCHEDULE AT CURRENT_TIMESTAMP + INTERVAL 1 YEAR DO INSERT INTO audit VALUES(99);",schema,&token).await?;}
            if driver==DbDriverType::Mysql {
                for _ in 0..14 { db.execute_schema_script("INSERT INTO z_parent(name,exact,bytes,created,flags,location) SELECT name,exact,bytes,created,flags,location FROM z_parent",schema,&token).await?; }
                db.execute_schema_script("CREATE SEQUENCE counter START WITH 17 INCREMENT BY 3; SELECT NEXT VALUE FOR counter;",schema,&token).await?;
            }
            let ddl=artifact(&db,schema,Task::Ddl).await?;let ddl_text=tokio::fs::read_to_string(ddl.file.path()).await?;assert!(ddl_text.contains("CREATE"));assert!(!ddl_text.contains("INSERT INTO \"export"));
            let dump=artifact(&db,schema,Task::Export(ExportOptions::default())).await?;
            let sql=tokio::fs::read_to_string(dump.file.path()).await?;assert!(sql.contains("INSERT INTO"));assert!(sql.contains("TRIGGER"));
            if driver==DbDriverType::Mysql{assert!(sql.contains("CREATE DEFINER")&&sql.contains("PROCEDURE"));}
            let drop=if driver==DbDriverType::Postgres{format!("DROP SCHEMA {} CASCADE",q(schema))}else{format!("DROP DATABASE {}",q(schema))};db.execute_schema_script(&drop,if driver==DbDriverType::Postgres{"public"}else{"nozhik"},&token).await?;
            // A dump creates its namespace; select the fixture database before executing it.
            db.execute_schema_script(&sql,if driver==DbDriverType::Postgres{"public"}else{"nozhik"},&token).await?;
            let rows=db.query(&format!("SELECT count(*) FROM {}.z_parent; SELECT count(*) FROM {}.audit; SELECT doubled FROM {}.a_child;",q(schema),q(schema),q(schema)),Some(schema),&token).await?;let expected=if driver==DbDriverType::Postgres{10005}else{16384};assert_eq!(rows[0].rows[0][0],DbValue::Int(expected));assert_eq!(rows[1].rows[0][0],DbValue::Int(expected));assert_eq!(rows[2].rows[0][0],DbValue::Int(2));
            let csv=artifact(&db,schema,Task::Export(ExportOptions {format:Format::Csv,table:"a_child".into(),..ExportOptions::default()})).await?;assert!(tokio::fs::read_to_string(csv.file.path()).await?.contains("doubled"));
            let values = if driver == DbDriverType::Postgres {
                db.query("SELECT exact::text,encode(bytes,'hex'),tags::text FROM z_parent ORDER BY id LIMIT 1; SELECT obj_description('z_parent'::regclass,'pg_class'); SELECT col_description('z_parent'::regclass,2);",Some(schema),&token).await?
            } else {
                db.query("SELECT CAST(exact AS CHAR),HEX(bytes),HEX(flags),ST_SRID(location),ST_AsText(location),CAST(created AS CHAR) FROM z_parent ORDER BY id LIMIT 1; SELECT NEXT VALUE FOR counter;",Some(schema),&token).await?
            };
            assert_eq!(values[0].rows[0][0],DbValue::Text("12345678901234567890.12345".into()));
            assert_eq!(values[0].rows[0][1],DbValue::Text(if driver==DbDriverType::Postgres {"00ff"}else{"00FF"}.into()));
            if driver==DbDriverType::Postgres {assert_eq!(values[1].rows[0][0],DbValue::Text("table comment".into()));assert_eq!(values[2].rows[0][0],DbValue::Text("column comment".into()));}
            else {assert_eq!(values[0].rows[0][2],DbValue::Text("81".into()));assert_eq!(values[0].rows[0][3],DbValue::Int(4326));assert_eq!(values[1].rows[0][0],DbValue::Int(3017));}
            let Output::Settings(old)=db.schema_task(schema,Task::Settings,&token).await? else {return Err("settings".into());};
            let mut new=old.clone();if driver==DbDriverType::Postgres {new.comment="updated comment".into();new.grants.push(Grant {user:"PUBLIC".into(),host:String::new(),privilege:"USAGE".into()});}else{new.collation="utf8mb4_bin".into();new.grants.push(Grant {user:"root".into(),host:"%".into(),privilege:"SELECT".into()});}
            db.schema_task(schema,Task::Modify(Box::new(old),Box::new(new)),&token).await?;
            let Output::Settings(saved)=db.schema_task(schema,Task::Settings,&token).await? else {return Err("settings".into());};
            if driver==DbDriverType::Postgres {assert_eq!(saved.comment,"updated comment");} else {assert_eq!(saved.collation,"utf8mb4_bin");}
            db.close().await;
        }Ok::<_,Box<dyn std::error::Error>>(())
    }).await??;
    Ok(())
}

#[tokio::test]
async fn cancelled_or_unfinished_sqlite_script_cannot_leak_transaction_or_progress_handler()
-> TestResult {
    tokio::time::timeout(Duration::from_secs(10), async {
        let directory=tempfile::tempdir()?;
        let db=Connection::connect(&ConnectionProfile {name:"cancel fixture".into(),..ConnectionProfile::default()},None,None).await?;
        let fresh=CancellationToken::new();db.query("CREATE TABLE items(value TEXT);",None,&fresh).await?;
        let path=directory.path().join("cancel.sql");
        tokio::fs::write(&path,"BEGIN; INSERT INTO items VALUES('cancelled'); WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT sum(x) FROM n; COMMIT;").await?;
        let cancel=CancellationToken::new();let stop=cancel.clone();
        let (result,())=tokio::join!(db.schema_task("main",Task::RunFile(path.clone()),&cancel),async move {tokio::time::sleep(Duration::from_millis(20)).await;stop.cancel();});
        assert!(matches!(result,Err(Error::Cancelled)));
        assert!(matches!(db.schema_task("main",Task::Settings,&fresh).await?,Output::Settings(_)));
        assert!(artifact(&db,"main",Task::Ddl).await?.preview.contains("CREATE TABLE items"));
        tokio::fs::write(&path,"BEGIN; INSERT INTO items VALUES('unfinished');").await?;
        assert!(db.schema_task("main",Task::RunFile(path),&fresh).await.is_err());
        let results=db.query("SELECT count(*) FROM items",None,&fresh).await?;assert_eq!(results[0].rows[0][0],DbValue::Int(0));
        db.close().await;Ok::<_,Box<dyn std::error::Error>>(())
    }).await??;
    Ok(())
}
