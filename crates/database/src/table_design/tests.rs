use super::*;
use crate::{
    CancellationToken, Connection, ConnectionProfile, DbValue,
    schema::{ExportOptions, Format, Output, Task},
};
use std::{sync::Arc, time::Duration};
type TestResult = std::result::Result<(), Box<dyn std::error::Error>>;
fn added_column(id: u64, name: &str) -> Element {
    Element::column(id, name.into())
}
fn element(id: u64, name: &str, data: Properties) -> Element {
    Element {
        id,
        name: name.into(),
        comment: String::new(),
        native_sql: String::new(),
        data,
    }
}
async fn generated(
    db: &Connection,
    schema: &str,
    task: Task,
) -> crate::Result<Arc<crate::schema::Artifact>> {
    match db
        .schema_task(schema, task, &CancellationToken::new())
        .await?
    {
        Output::Artifact(a) => Ok(a),
        _ => Err(invalid("Artifact expected")),
    }
}
async fn export_text(db: &Connection, schema: &str, task: Task) -> crate::Result<String> {
    let a = generated(db, schema, task).await?;
    match db
        .schema_task(schema, Task::Clipboard(a), &CancellationToken::new())
        .await?
    {
        Output::Clipboard(s) => Ok(s),
        _ => Err(invalid("Clipboard expected")),
    }
}
#[allow(
    clippy::too_many_lines,
    reason = "End-to-end mutation, rollback and scoped dump/restore share the same table state"
)]
async fn exercise(db: &Connection, schema: &str) -> TestResult {
    let token = CancellationToken::new();
    let q = |s: &str| quote_identifier(db.driver, s);
    let target = format!("{}.{}", q(schema), q("design_items"));
    let old = db.table_definition(schema, "design_items").await?;
    assert_eq!(migration_sql(&old, &old)?, "");
    let mut rename = old.clone();
    rename
        .elements
        .iter_mut()
        .find(|e| e.name == "name")
        .ok_or("name column")?
        .name = "label".into();
    let renamed = db.modify_table(&old, &rename, &token).await?;
    assert!(renamed.elements.iter().any(|e| e.name == "label"));
    let mut draft = renamed.clone();
    draft
        .elements
        .retain(|e| e.name != "obsolete" && e.name != "design_name_idx");
    let mut next = renamed.next_id();
    let mut c = added_column(next, "required_number");
    next += 1;
    if let Properties::Column(c) = &mut c.data {
        c.not_null = true;
        c.default_sql = "7".into();
    }
    draft.elements.push(c);
    let c = added_column(next, "parent_id");
    next += 1;
    draft.elements.push(c);
    let p = |name: &str| IndexPart {
        expression: q(name),
        ..IndexPart::default()
    };
    draft.elements.push(element(
        next,
        "design_label_unique",
        Properties::Key(Key {
            kind: KeyKind::Unique,
            columns: vec![p("label")],
            ..Key::default()
        }),
    ));
    next += 1;
    draft.elements.push(element(
        next,
        "design_parent_fk",
        Properties::Key(Key {
            kind: KeyKind::Foreign,
            columns: vec![p("parent_id")],
            referenced_schema: if db.driver == DbDriverType::Sqlite {
                String::new()
            } else {
                schema.into()
            },
            referenced_table: "design_parent".into(),
            referenced_columns: vec!["id".into()],
            on_delete: "CASCADE".into(),
            ..Key::default()
        }),
    ));
    next += 1;
    draft.elements.push(element(
        next,
        "design_amount_check",
        Properties::Key(Key {
            kind: KeyKind::Check,
            expression: format!("{} >= 0", q("amount")),
            ..Key::default()
        }),
    ));
    next += 1;
    draft.elements.push(element(
        next,
        "design_amount_idx",
        Properties::Index(Index {
            columns: vec![p("amount")],
            ..Index::default()
        }),
    ));
    if let Some(e) = draft.elements.iter_mut().find(|e| e.name == "amount")
        && let Properties::Column(c) = &mut e.data
    {
        c.data_type = "BIGINT".into();
    }
    if db.driver != DbDriverType::Sqlite {
        draft.comment = "edited table's comment".into();
    }
    let sql = migration_sql(&renamed, &draft)?;
    assert!(sql.contains("required_number"));
    assert!(sql.contains("design_parent_fk"));
    let applied = db.modify_table(&renamed, &draft, &token).await?;
    assert!(db.modify_table(&renamed, &draft, &token).await.is_err());
    assert!(
        applied
            .elements
            .iter()
            .any(|e| e.name == "design_amount_idx")
    );
    assert!(!applied.elements.iter().any(|e| e.name == "obsolete"));
    let data=db.query(&format!("SELECT count(*),sum(required_number),sum(amount) FROM {target}; SELECT count(*) FROM {}.design_audit;",q(schema)),Some(schema),&token).await?;
    assert_eq!(data[0].rows[0][0], DbValue::Int(3));
    assert_eq!(data[0].rows[0][1].to_string(), "21");
    assert_eq!(data[1].rows[0][0], DbValue::Int(3));
    let mut invalid = applied.clone();
    invalid.elements.push(element(
        applied.next_id(),
        "must_fail",
        Properties::Key(Key {
            kind: KeyKind::Check,
            expression: "amount < 0".into(),
            ..Key::default()
        }),
    ));
    assert!(db.modify_table(&applied, &invalid, &token).await.is_err());
    let current = db.table_definition(schema, "design_items").await?;
    assert!(current.same_schema(&applied));
    db.query(
        &format!("INSERT INTO {target}(label,amount,parent_id) VALUES('last',4,1)"),
        Some(schema),
        &token,
    )
    .await?;
    let ddl = export_text(db, schema, Task::TableDdl("design_items".into())).await?;
    assert!(ddl.contains("design_amount_idx"));
    assert!(!ddl.contains("CREATE TABLE design_parent"));
    assert!(!ddl.contains("CREATE TABLE design_audit"));
    assert!(!ddl.contains("CREATE DATABASE"));
    let dump = export_text(
        db,
        schema,
        Task::Export(ExportOptions {
            table: "design_items".into(),
            only_table: true,
            ..ExportOptions::default()
        }),
    )
    .await?;
    assert!(dump.contains("INSERT INTO"));
    assert!(!dump.contains("CREATE DATABASE"));
    assert!(!dump.contains(&format!("CREATE TABLE {}.{}", q(schema), q("design_audit"))));
    let csv = export_text(
        db,
        schema,
        Task::Export(ExportOptions {
            format: Format::Csv,
            table: "design_items".into(),
            only_table: true,
            ..ExportOptions::default()
        }),
    )
    .await?;
    assert!(
        csv.lines()
            .next()
            .is_some_and(|l| l.contains("required_number")
                && l.contains("label")
                && !l.contains("obsolete"))
    );
    assert_eq!(csv.lines().count(), 5);
    let data_only = export_text(
        db,
        schema,
        Task::Export(ExportOptions {
            table: "design_items".into(),
            only_table: true,
            ddl: false,
            ..ExportOptions::default()
        }),
    )
    .await?;
    assert!(!data_only.contains("CREATE TABLE"));
    let drop = if db.driver == DbDriverType::Postgres {
        format!("DROP TABLE {target} CASCADE")
    } else {
        format!("DROP TABLE {target}")
    };
    db.execute_schema_script(&drop, schema, &token).await?;
    db.execute_schema_script(&dump, schema, &token).await?;
    let rows=db.query(&format!("SELECT count(*),sum(required_number) FROM {target}; SELECT count(*) FROM {}.design_audit;",q(schema)),Some(schema),&token).await?;
    assert_eq!(rows[0].rows[0][0], DbValue::Int(4));
    assert_eq!(rows[0].rows[0][1].to_string(), "28");
    assert_eq!(rows[1].rows[0][0], DbValue::Int(4));
    Ok(())
}
#[tokio::test]
async fn sqlite_rebuild_preserves_rows_dependencies_and_counters() -> TestResult {
    tokio::time::timeout(Duration::from_secs(20),async{
    let db=Connection::connect(&ConnectionProfile{name:"design".into(),..ConnectionProfile::default()},None,None).await?;
    db.query("CREATE TABLE design_parent(id INTEGER PRIMARY KEY); INSERT INTO design_parent VALUES(1); CREATE TABLE design_items(id INTEGER PRIMARY KEY AUTOINCREMENT,name TEXT,amount INTEGER,obsolete TEXT); CREATE TABLE design_audit(id INTEGER); CREATE TRIGGER design_log AFTER INSERT ON design_items BEGIN INSERT INTO design_audit VALUES(NEW.id); END; CREATE INDEX design_name_idx ON design_items(name); CREATE VIEW design_view AS SELECT name FROM design_items; INSERT INTO design_items(name,amount) VALUES('one',1),('two',2),('three',3); UPDATE sqlite_sequence SET seq=2000 WHERE name='design_items';",None,&CancellationToken::new()).await?;
    exercise(&db,"main").await?;
    let result=db.query("SELECT max(id) FROM design_items; PRAGMA foreign_keys; SELECT label FROM design_view ORDER BY label;",None,&CancellationToken::new()).await?;assert_eq!(result[0].rows[0][0],DbValue::Int(2001));assert_eq!(result[1].rows[0][0],DbValue::Int(1));assert_eq!(result[2].rows.len(),4);
    db.close().await;Ok::<_,Box<dyn std::error::Error>>(())
}).await??;
    Ok(())
}
#[tokio::test]
#[ignore = "Requires disposable PostgreSQL/MariaDB on NOZHIK_DESIGN_PG_PORT/NOZHIK_DESIGN_MYSQL_PORT"]
async fn system_table_design_and_scoped_exports() -> TestResult {
    tokio::time::timeout(Duration::from_secs(90),async{
    for driver in [DbDriverType::Postgres,DbDriverType::Mysql]{let port=std::env::var(if driver==DbDriverType::Postgres{"NOZHIK_DESIGN_PG_PORT"}else{"NOZHIK_DESIGN_MYSQL_PORT"})?.parse()?;let db=Connection::connect(&ConnectionProfile{name:"design fixture".into(),driver,host:"127.0.0.1".into(),port,user:if driver==DbDriverType::Postgres{"postgres"}else{"root"}.into(),database:"nozhik".into(),..ConnectionProfile::default()},Some(&nozhik_remote_ssh::credentials::Secret::new("nozhik-fixture-only".into())),None).await?;
        let token=CancellationToken::new();let schema=if driver==DbDriverType::Postgres{"public"}else{"nozhik"};
        let setup=if driver==DbDriverType::Postgres{"DROP TABLE IF EXISTS design_items CASCADE; DROP TABLE IF EXISTS design_parent CASCADE; DROP TABLE IF EXISTS design_audit; CREATE TABLE design_parent(id INTEGER PRIMARY KEY); INSERT INTO design_parent VALUES(1); CREATE TABLE design_items(id BIGINT GENERATED ALWAYS AS IDENTITY PRIMARY KEY,name TEXT,amount INTEGER,obsolete TEXT); CREATE TABLE design_audit(id BIGINT); CREATE OR REPLACE FUNCTION design_log_fn() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN INSERT INTO design_audit VALUES(NEW.id); RETURN NEW; END $$; CREATE TRIGGER design_log AFTER INSERT ON design_items FOR EACH ROW EXECUTE FUNCTION design_log_fn(); CREATE INDEX design_name_idx ON design_items(name); INSERT INTO design_items(name,amount) VALUES('one',1),('two',2),('three',3);"}else{"DROP TABLE IF EXISTS design_items; DROP TABLE IF EXISTS design_parent; DROP TABLE IF EXISTS design_audit; CREATE TABLE design_parent(id INTEGER PRIMARY KEY); INSERT INTO design_parent VALUES(1); CREATE TABLE design_items(id BIGINT PRIMARY KEY AUTO_INCREMENT,name VARCHAR(100),amount INTEGER,obsolete TEXT); CREATE TABLE design_audit(id BIGINT); CREATE TRIGGER design_log AFTER INSERT ON design_items FOR EACH ROW INSERT INTO design_audit VALUES(NEW.id); CREATE INDEX design_name_idx ON design_items(name); INSERT INTO design_items(name,amount) VALUES('one',1),('two',2),('three',3);"};db.execute_schema_script(setup,schema,&token).await?;exercise(&db,schema).await?;edit_existing_objects(&db,schema).await?;db.close().await;
    }Ok::<_,Box<dyn std::error::Error>>(())
}).await??;
    Ok(())
}

#[tokio::test]
async fn sqlite_dependency_failure_and_cancel_restore_session_state() -> TestResult {
    tokio::time::timeout(Duration::from_secs(20),async{
        let db=Connection::connect(&ConnectionProfile{name:"rollback fixture".into(),..ConnectionProfile::default()},None,None).await?;let token=CancellationToken::new();
        db.query("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT); CREATE INDEX value_idx ON items(value); CREATE TRIGGER value_log AFTER INSERT ON items BEGIN SELECT NEW.value; END; WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000) INSERT INTO items SELECT x,'value' FROM n; PRAGMA foreign_keys=OFF; PRAGMA legacy_alter_table=ON;",None,&token).await?;
        let old=db.table_definition("main","items").await?;let mut invalid=old.clone();invalid.elements.retain(|e|e.name!="value");assert!(migration_sql(&old,&invalid).is_err());
        let mut changed=old.clone();changed.strict=true;let preview=migration_sql(&old,&changed)?;assert!(preview.contains("PRAGMA foreign_keys=0"));assert!(preview.contains("PRAGMA legacy_alter_table=1"));
        let cancel=CancellationToken::new();let stop=cancel.clone();let(result,())=tokio::join!(db.modify_table(&old,&changed,&cancel),async move{tokio::time::sleep(Duration::from_millis(10)).await;stop.cancel();});assert!(matches!(result,Err(crate::Error::Cancelled)));
        let current=db.table_definition("main","items").await?;assert!(current.same_schema(&old));let result=db.query("SELECT count(*) FROM items; PRAGMA foreign_keys; PRAGMA legacy_alter_table;",None,&token).await?;assert_eq!(result[0].rows[0][0],DbValue::Int(100_000));assert_eq!(result[1].rows[0][0],DbValue::Int(0));assert_eq!(result[2].rows[0][0],DbValue::Int(1));db.close().await;Ok::<_,Box<dyn std::error::Error>>(())
    }).await??;
    Ok(())
}

#[test]
fn native_catalog_clauses_preserve_quotes_generated_attributes_and_index_options() -> TestResult {
    let generated = sql::column(
        1,
        "\"generated name\" INTEGER GENERATED ALWAYS AS (length('it''s, (valid)')) STORED NOT NULL CHECK(\"generated name\" > 0)",
    )?;
    let Properties::Column(column) = generated.data else {
        return Err("column".into());
    };
    assert!(column.not_null && column.stored);
    assert!(column.extra_sql.contains("CHECK"));
    assert!(column.generated_sql.contains("it''s"));
    let index = sql::index(
        2,
        "CREATE INDEX index_name ON public.items USING btree (label COLLATE \"pg_catalog\".\"default\" varchar_pattern_ops DESC NULLS LAST) INCLUDE (amount) WHERE amount > 0",
    )?;
    let Properties::Index(index) = index.data else {
        return Err("index".into());
    };
    assert_eq!(index.columns[0].expression, "label");
    assert_eq!(index.columns[0].collation, "\"pg_catalog\".\"default\"");
    assert_eq!(index.columns[0].operator_class, "varchar_pattern_ops");
    assert_eq!(index.columns[0].order, "DESC NULLS LAST");
    assert_eq!(index.include_columns, vec!["amount"]);
    assert_eq!(index.condition, "amount > 0");
    let key = sql::key(
        3,
        "CONSTRAINT deferred_key UNIQUE (label) deferrable initially deferred",
    )?;
    let Properties::Key(key) = key.data else {
        return Err("key".into());
    };
    assert!(key.deferrable && key.deferred);
    assert_eq!(key.extra_sql, "");
    assert!(sql::fragment("E'backslash\\\\ and quote\\''").is_ok());
    assert!(sql::fragment("integer; DROP TABLE items").is_err());
    Ok(())
}

async fn edit_existing_objects(db: &Connection, schema: &str) -> TestResult {
    let token = CancellationToken::new();
    let mut old = db.table_definition(schema, "design_items").await?;
    let mut draft = old.clone();
    for e in &mut draft.elements {
        match &mut e.data {
            Properties::Key(k) if e.name == "design_label_unique" => {
                k.columns.push(IndexPart {
                    expression: quote_identifier(db.driver, "amount"),
                    ..IndexPart::default()
                });
                if db.driver == DbDriverType::Postgres {
                    k.deferrable = true;
                    k.deferred = true;
                }
            }
            Properties::Key(k) if k.kind == KeyKind::Foreign => k.on_delete = "RESTRICT".into(),
            Properties::Index(i) if e.name == "design_amount_idx" => {
                e.name = "design_amount_desc".into();
                i.columns[0].order = "DESC".into();
                e.comment = if db.driver == DbDriverType::Sqlite {
                    String::new()
                } else {
                    "sorted amount".into()
                };
            }
            Properties::Column(c) if !c.identity.is_empty() => {
                c.identity_options = c.identity_options.replace("CACHE 1", "CACHE 2");
            }
            _ => {}
        }
    }
    if db.driver == DbDriverType::Mysql {
        draft.row_format = "COMPACT".into();
        draft.collation = "utf8mb4_bin".into();
    }
    if db.driver == DbDriverType::Postgres {
        draft.storage_options = "fillfactor=80".into();
        draft.grants.push(TableGrant {
            user: "PUBLIC".into(),
            privilege: "SELECT".into(),
            ..TableGrant::default()
        });
    }
    let applied = db.modify_table(&old, &draft, &token).await?;
    assert!(
        applied
            .elements
            .iter()
            .any(|e| e.name == "design_amount_desc")
    );
    if db.driver == DbDriverType::Postgres {
        db.execute_schema_script("CREATE CONSTRAINT TRIGGER design_deferred AFTER UPDATE ON design_items DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION design_log_fn();",schema,&token).await?;
        old = db.table_definition(schema, "design_items").await?;
        draft = old.clone();
        if let Some(e) = draft
            .elements
            .iter_mut()
            .find(|e| e.name == "design_deferred")
            && let Properties::Trigger(t) = &mut e.data
        {
            assert!(t.constraint && t.deferrable && t.deferred);
            t.condition = "NEW.amount > 0".into();
        }
        let applied = db.modify_table(&old, &draft, &token).await?;
        assert!(applied.elements.iter().any(|e|matches!(&e.data,Properties::Trigger(t) if e.name=="design_deferred"&&t.constraint&&t.deferred)));
        let ddl = export_text(db, schema, Task::TableDdl("design_items".into())).await?;
        assert!(ddl.contains("COMMENT ON INDEX public.design_amount_desc IS 'sorted amount'"));
    } else if db.driver == DbDriverType::Mysql {
        db.execute_schema_script("SET SQL_MODE=''; CREATE TRIGGER design_empty_mode BEFORE UPDATE ON design_items FOR EACH ROW BEGIN SET NEW.amount=NEW.amount; END;",schema,&token).await?;
        old = db.table_definition(schema, "design_items").await?;
        draft = old.clone();
        if let Some(e) = draft
            .elements
            .iter_mut()
            .find(|e| e.name == "design_empty_mode")
            && let Properties::Trigger(t) = &mut e.data
        {
            assert!(t.preserve_sql_mode);
            assert_eq!(t.sql_mode, "");
            t.body_sql = "BEGIN SET NEW.amount=ABS(NEW.amount); END".into();
        }
        let applied = db.modify_table(&old, &draft, &token).await?;
        assert!(applied.elements.iter().any(|e|matches!(&e.data,Properties::Trigger(t) if e.name=="design_empty_mode"&&t.preserve_sql_mode&&t.sql_mode.is_empty())));
    }
    Ok(())
}

#[test]
fn create_table_plans_and_atomic_sqlite_execution() -> TestResult {
    let runtime = tokio::runtime::Runtime::new()?;
    runtime.block_on(async {
        let db = Connection::connect(
            &ConnectionProfile {
                name: "create fixture".into(),
                ..ConnectionProfile::default()
            },
            None,
            None,
        )
        .await?;
        let token = CancellationToken::new();
        let mut draft = Definition {
            driver: DbDriverType::Sqlite,
            schema: "main".into(),
            name: "new \"table".into(),
            strict: true,
            elements: vec![
                added_column(1, "id"),
                element(
                    2,
                    "value",
                    Properties::Column(Column {
                        data_type: "TEXT".into(),
                        default_sql: "'initial'".into(),
                        ..Column::default()
                    }),
                ),
                element(
                    3,
                    "by_value",
                    Properties::Index(Index {
                        columns: vec![IndexPart {
                            expression: "value".into(),
                            ..IndexPart::default()
                        }],
                        ..Index::default()
                    }),
                ),
            ],
            ..Definition::default()
        };
        let preview = creation_sql(&draft)?;
        assert!(preview.contains("CREATE TABLE \"main\".\"new \"\"table\""));
        assert!(preview.contains("STRICT"));
        let Output::Table(created) = db
            .schema_task("main", Task::CreateTable(Box::new(draft.clone())), &token)
            .await?
        else {
            return Err("table expected".into());
        };
        assert!(created.elements.iter().any(|e| e.name == "by_value"));
        assert_eq!(migration_sql(&created, &created)?, "");
        assert!(db.create_table(&draft, &token).await.is_err());
        draft.name = "rolled_back".into();
        // Duplicate global index name fails after CREATE TABLE; the transaction must roll back.
        assert!(db.create_table(&draft, &token).await.is_err());
        assert!(db.table_definition("main", "rolled_back").await.is_err());
        draft
            .elements
            .retain(|e| !matches!(e.data, Properties::Index(_)));
        let cancelled = CancellationToken::new();
        cancelled.cancel();
        assert!(db.create_table(&draft, &cancelled).await.is_err());
        assert!(db.table_definition("main", "rolled_back").await.is_err());
        draft.elements.clear();
        assert!(creation_sql(&draft).is_err());
        db.close().await;
        Ok::<_, Box<dyn std::error::Error>>(())
    })?;
    for driver in [DbDriverType::Postgres, DbDriverType::Mysql] {
        let draft = Definition {
            driver,
            schema: "schema".into(),
            name: "created".into(),
            comment: "table comment".into(),
            elements: vec![added_column(1, "id")],
            ..Definition::default()
        };
        let sql = creation_sql(&draft)?;
        assert!(sql.contains("CREATE TABLE"));
        assert!(sql.contains("table comment"));
        assert!(!sql.contains("ADD COLUMN"));
        assert!(!sql.contains("DROP TABLE"));
    }
    Ok(())
}
