//! PostgreSQL catalog serialization and server-side, type-preserving row literals.
use super::{Artifact, Connection, Error, ExportOptions, Pool, Result, finish, temporary};
use crate::{DbDriverType, connection::quote_identifier};
use futures_util::TryStreamExt;
use sqlx::{Connection as _, Row};
use std::fmt::Write as _;
use tokio::io::AsyncWriteExt;

mod metadata;

type Writer = tokio::io::BufWriter<tokio::fs::File>;
type Pg = sqlx::PgConnection;
fn q(s: &str) -> String {
    quote_identifier(DbDriverType::Postgres, s)
}
fn text(s: &str) -> String {
    format!("E'{}'", s.replace('\\', "\\\\").replace('\'', "''"))
}
async fn write(out: &mut Writer, sql: &str) -> Result<()> {
    out.write_all(sql.as_bytes()).await?;
    out.write_all(b";\n").await?;
    Ok(())
}
// Extension members are recreated by CREATE EXTENSION, never independently.
const RELATIONS: &str = "SELECT c.oid::bigint AS id,c.relname::text AS name,c.relkind::text AS kind,c.relpersistence::text AS persistence,c.relispartition, pg_get_expr(c.relpartbound,c.oid,true) AS bound,pg_get_partkeydef(c.oid) AS partition_key,COALESCE(array_to_string(c.reloptions,', '),'') AS options, c.relrowsecurity,c.relforcerowsecurity FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind IN ('r','p','v','m','f') AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='e') ORDER BY c.oid";
impl Connection {
    pub(super) async fn postgres_dump(
        &self,
        schema: &str,
        options: Option<&ExportOptions>,
        ddl_only: bool,
    ) -> Result<Artifact> {
        let Pool::Postgres(pool) = &self.pool else {
            return Err(Error::Invalid("PostgreSQL required".into()));
        };
        let ddl = options.is_none_or(|o| o.ddl);
        let file = temporary().await?;
        let mut out = Writer::new(tokio::fs::File::from_std(file.reopen()?));
        let mut connection = pool.acquire().await?;
        connection.close_on_drop();
        let mut tx = connection.begin().await?;
        sqlx::raw_sql(&format!("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY; SET LOCAL search_path TO {},pg_catalog; SET LOCAL TIME ZONE 'UTC'; SET LOCAL DateStyle='ISO'; SET LOCAL extra_float_digits=3; SET LOCAL IntervalStyle='postgres'; SET LOCAL row_security=off;",q(schema))).execute(&mut *tx).await?;
        let exists: bool =
            sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM pg_namespace WHERE nspname=$1)")
                .bind(schema)
                .fetch_one(&mut *tx)
                .await?;
        if !exists {
            return Err(Error::Invalid("Schema no longer exists".into()));
        }
        out.write_all(b"-- Nozhik SQL export (PostgreSQL)\nSET standard_conforming_strings=on;\nSET client_encoding='UTF8';\nSET check_function_bodies=false;\nSET TIME ZONE 'UTC';\nSET DateStyle='ISO';\n").await?;
        let relations = sqlx::query(RELATIONS)
            .bind(schema)
            .fetch_all(&mut *tx)
            .await?;
        let sequences = sequence_rows(&mut tx, schema).await?;
        if ddl {
            write(
                &mut out,
                &format!("CREATE SCHEMA IF NOT EXISTS {}", q(schema)),
            )
            .await?;
            write(
                &mut out,
                &format!("SET search_path TO {},pg_catalog", q(schema)),
            )
            .await?;
            check_supported(&mut tx, schema).await?;
            extensions(&mut tx, &mut out, schema).await?;
            collations(&mut tx, &mut out, schema).await?;
            types(&mut tx, &mut out, schema).await?;
            for row in &sequences {
                if row.try_get::<bool, _>("identity")? {
                    continue;
                }
                write(&mut out,&format!("CREATE SEQUENCE {}.{} AS {} INCREMENT BY {} MINVALUE {} MAXVALUE {} START WITH {} CACHE {} {}",q(schema),q(&row.try_get::<String,_>("name")?),row.try_get::<String,_>("kind")?,row.try_get::<i64,_>("seqincrement")?,row.try_get::<i64,_>("seqmin")?,row.try_get::<i64,_>("seqmax")?,row.try_get::<i64,_>("seqstart")?,row.try_get::<i64,_>("seqcache")?,if row.try_get::<bool,_>("seqcycle")? {"CYCLE"} else {"NO CYCLE"})).await?;
            }
            functions(&mut tx, &mut out, schema, false).await?;
            tables(&mut tx, &mut out, schema, &relations).await?;
            functions(&mut tx, &mut out, schema, true).await?;
        }
        if !ddl_only {
            write(&mut out, "BEGIN").await?;
            for row in &relations {
                if row.try_get::<String, _>("kind")? != "r" {
                    continue;
                }
                data(
                    &mut tx,
                    &mut out,
                    schema,
                    &row.try_get::<String, _>("name")?,
                    row.try_get("id")?,
                )
                .await?;
            }
            write(&mut out, "COMMIT").await?;
            for row in &sequences {
                let name: String = row.try_get("name")?;
                let state = sqlx::query(&format!(
                    "SELECT last_value,is_called FROM {}.{}",
                    q(schema),
                    q(&name)
                ))
                .fetch_one(&mut *tx)
                .await?;
                write(
                    &mut out,
                    &format!(
                        "SELECT pg_catalog.setval({}::regclass, {}, {})",
                        text(&format!("{}.{}", q(schema), q(&name))),
                        state.try_get::<i64, _>("last_value")?,
                        state.try_get::<bool, _>("is_called")?
                    ),
                )
                .await?;
            }
        }
        if ddl {
            views(&mut tx, &mut out, schema, &relations, !ddl_only).await?;
            post_data(&mut tx, &mut out, schema, &relations).await?;
            ownership(&mut tx, &mut out, schema, &sequences).await?;
        }
        out.flush().await?;
        tx.commit().await?;
        finish(file).await
    }
}
async fn extensions(c: &mut Pg, out: &mut Writer, schema: &str) -> Result<()> {
    for row in sqlx::query("SELECT e.extname::text,e.extversion::text FROM pg_extension e JOIN pg_namespace n ON n.oid=e.extnamespace WHERE n.nspname=$1 ORDER BY e.extname").bind(schema).fetch_all(&mut *c).await? {
        write(out,&format!("CREATE EXTENSION IF NOT EXISTS {} WITH SCHEMA {} VERSION {}",q(&row.try_get::<String,_>("extname")?),q(schema),text(&row.try_get::<String,_>("extversion")?))).await?;
    }
    Ok(())
}
async fn sequence_rows(c: &mut Pg, schema: &str) -> Result<Vec<sqlx::postgres::PgRow>> {
    Ok(sqlx::query("SELECT c.oid::bigint AS id,c.relname::text AS name,format_type(s.seqtypid,NULL) AS kind,s.seqstart,s.seqincrement,s.seqmax,s.seqmin,s.seqcache,s.seqcycle, EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='i') AS identity FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_sequence s ON s.seqrelid=c.oid WHERE n.nspname=$1 AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='e') ORDER BY c.relname").bind(schema).fetch_all(c).await?)
}
#[allow(
    clippy::too_many_lines,
    reason = "Native type categories use separate catalog forms but one ordered namespace pass"
)]
async fn types(c: &mut Pg, out: &mut Writer, schema: &str) -> Result<()> {
    let rows=sqlx::query("SELECT t.oid::bigint AS id,t.typname::text AS name,t.typtype::text AS kind,t.typrelid::bigint AS relation,t.typnotnull,format_type(t.typbasetype,t.typtypmod) AS base,pg_get_expr(t.typdefaultbin,0) AS default_value FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace LEFT JOIN pg_class r ON r.oid=t.typrelid WHERE n.nspname=$1 AND t.typelem=0 AND (t.typrelid=0 OR r.relkind='c') AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_type'::regclass AND d.objid=t.oid AND d.deptype='e') ORDER BY t.oid").bind(schema).fetch_all(&mut *c).await?;
    for row in rows {
        let id: i64 = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let full = format!("{}.{}", q(schema), q(&name));
        match row.try_get::<String, _>("kind")?.as_str() {
            "e" => {
                let labels:Vec<String>=sqlx::query_scalar("SELECT enumlabel::text FROM pg_enum WHERE enumtypid=$1::bigint::oid ORDER BY enumsortorder").bind(id).fetch_all(&mut *c).await?;
                write(
                    out,
                    &format!(
                        "CREATE TYPE {full} AS ENUM ({})",
                        labels
                            .iter()
                            .map(|s| text(s))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                )
                .await?;
            }
            "d" => {
                let constraints:Vec<(String,String)>=sqlx::query_as("SELECT conname::text,pg_get_constraintdef(oid,true) FROM pg_constraint WHERE contypid=$1::bigint::oid ORDER BY conname").bind(id).fetch_all(&mut *c).await?;
                let mut definition = format!(
                    "CREATE DOMAIN {full} AS {}",
                    row.try_get::<String, _>("base")?
                );
                if let Some(default) = row.try_get::<Option<String>, _>("default_value")? {
                    write!(definition, " DEFAULT {default}").ok();
                }
                if row.try_get::<bool, _>("typnotnull")? {
                    definition.push_str(" NOT NULL");
                }
                for (name, sql) in constraints {
                    write!(definition, " CONSTRAINT {} {sql}", q(&name)).ok();
                }
                write(out, &definition).await?;
            }
            "c" => {
                let columns = columns(c, row.try_get("relation")?).await?;
                let fields = columns
                    .iter()
                    .map(|r| {
                        Ok(format!(
                            "{} {}",
                            q(&r.try_get::<String, _>("name")?),
                            r.try_get::<String, _>("kind")?
                        ))
                    })
                    .collect::<Result<Vec<_>>>()?;
                write(
                    out,
                    &format!("CREATE TYPE {full} AS ({})", fields.join(", ")),
                )
                .await?;
            }
            kind => {
                return Err(Error::Invalid(format!(
                    "Cannot export user-defined PostgreSQL type {schema}.{name} ({kind}) losslessly yet"
                )));
            }
        }
    }
    Ok(())
}
async fn functions(c: &mut Pg, out: &mut Writer, schema: &str, late: bool) -> Result<()> {
    let rows=sqlx::query("SELECT p.oid::bigint AS id,p.prokind::text AS kind,EXISTS(SELECT 1 FROM pg_type t JOIN pg_class r ON r.oid=t.typrelid WHERE r.relkind IN ('r','p','v','m','f') AND (t.oid=p.prorettype OR t.oid=ANY(p.proargtypes::oid[]))) AS late FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_proc'::regclass AND d.objid=p.oid AND d.deptype='e') ORDER BY p.oid").bind(schema).fetch_all(&mut *c).await?;
    for row in rows {
        if row.try_get::<bool, _>("late")? != late {
            continue;
        }
        if row.try_get::<String, _>("kind")? == "a" {
            return Err(Error::Invalid(
                "Custom PostgreSQL aggregates cannot yet be exported losslessly".into(),
            ));
        }
        let definition: String = sqlx::query_scalar("SELECT pg_get_functiondef($1::bigint::oid)")
            .bind(row.try_get::<i64, _>("id")?)
            .fetch_one(&mut *c)
            .await?;
        write(out, definition.trim_end_matches([';', '\n'])).await?;
    }
    Ok(())
}
async fn columns(c: &mut Pg, id: i64) -> Result<Vec<sqlx::postgres::PgRow>> {
    Ok(sqlx::query("SELECT a.attname::text AS name,format_type(a.atttypid,a.atttypmod) AS kind,a.attnotnull AS required,a.attidentity::text AS identity,a.attgenerated::text AS generated,a.attislocal,pg_get_expr(d.adbin,d.adrelid) AS default_value,CASE WHEN a.attcollation<>t.typcollation THEN quote_ident(n.nspname)||'.'||quote_ident(coll.collname) END AS collation FROM pg_attribute a JOIN pg_type t ON t.oid=a.atttypid LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum LEFT JOIN pg_collation coll ON coll.oid=a.attcollation LEFT JOIN pg_namespace n ON n.oid=coll.collnamespace WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum").bind(id).fetch_all(c).await?)
}
async fn column_definition(
    c: &mut Pg,
    table_id: i64,
    row: &sqlx::postgres::PgRow,
) -> Result<String> {
    let name: String = row.try_get("name")?;
    let mut definition = format!("{} {}", q(&name), row.try_get::<String, _>("kind")?);
    if let Some(collation) = row.try_get::<Option<String>, _>("collation")? {
        write!(definition, " COLLATE {collation}").ok();
    }
    let identity: String = row.try_get("identity")?;
    let generated: String = row.try_get("generated")?;
    let default: Option<String> = row.try_get("default_value")?;
    if !identity.is_empty() {
        let seq=sqlx::query("SELECT n.nspname::text AS schema,c.relname::text AS name,s.* FROM pg_sequence s JOIN pg_class c ON c.oid=s.seqrelid JOIN pg_namespace n ON n.oid=c.relnamespace JOIN pg_depend d ON d.objid=c.oid AND d.classid='pg_class'::regclass JOIN pg_attribute a ON a.attrelid=d.refobjid AND a.attnum=d.refobjsubid WHERE d.refobjid=$1::bigint::oid AND d.deptype='i' AND a.attname=$2").bind(table_id).bind(&name).fetch_one(&mut *c).await?;
        write!(definition, " GENERATED {} AS IDENTITY (SEQUENCE NAME {}.{} START WITH {} INCREMENT BY {} MINVALUE {} MAXVALUE {} CACHE {} {})",if identity=="a" {"ALWAYS"} else {"BY DEFAULT"},q(&seq.try_get::<String,_>("schema")?),q(&seq.try_get::<String,_>("name")?),seq.try_get::<i64,_>("seqstart")?,seq.try_get::<i64,_>("seqincrement")?,seq.try_get::<i64,_>("seqmin")?,seq.try_get::<i64,_>("seqmax")?,seq.try_get::<i64,_>("seqcache")?,if seq.try_get::<bool,_>("seqcycle")? {"CYCLE"} else {"NO CYCLE"}).ok();
    } else if !generated.is_empty() {
        write!(
            definition,
            " GENERATED ALWAYS AS ({}) {}",
            default.unwrap_or_default(),
            if generated == "v" {
                "VIRTUAL"
            } else {
                "STORED"
            }
        )
        .ok();
    } else if let Some(default) = default {
        write!(definition, " DEFAULT {default}").ok();
    }
    if row.try_get::<bool, _>("required")? {
        definition.push_str(" NOT NULL");
    }
    Ok(definition)
}
async fn tables(
    c: &mut Pg,
    out: &mut Writer,
    schema: &str,
    relations: &[sqlx::postgres::PgRow],
) -> Result<()> {
    let mut waiting = relations
        .iter()
        .filter(|r| {
            r.try_get::<String, _>("kind")
                .is_ok_and(|k| ["r", "p", "f"].contains(&k.as_str()))
        })
        .collect::<Vec<_>>();
    let mut emitted = std::collections::HashSet::<i64>::new();
    while !waiting.is_empty() {
        let before = waiting.len();
        let mut next = Vec::new();
        for row in waiting {
            let id: i64 = row.try_get("id")?;
            let name: String = row.try_get("name")?;
            if row.try_get::<String, _>("kind")? == "f" {
                return Err(Error::Invalid("Foreign tables require external server/user mappings and cannot yet be exported losslessly".into()));
            }
            let parents:Vec<(i64,String,String)>=sqlx::query_as("SELECT p.oid::bigint,n.nspname::text,p.relname::text FROM pg_inherits i JOIN pg_class p ON p.oid=i.inhparent JOIN pg_namespace n ON n.oid=p.relnamespace WHERE i.inhrelid=$1::bigint::oid ORDER BY i.inhseqno").bind(id).fetch_all(&mut *c).await?;
            if parents
                .iter()
                .any(|(id, s, _)| s == schema && !emitted.contains(id))
            {
                next.push(row);
                continue;
            }
            let full = format!("{}.{}", q(schema), q(&name));
            let mut definition;
            if row.try_get::<bool, _>("relispartition")? {
                let (_, s, n) = parents
                    .first()
                    .ok_or_else(|| Error::Invalid("Partition parent missing".into()))?;
                definition = format!(
                    "CREATE TABLE {full} PARTITION OF {}.{} {}",
                    q(s),
                    q(n),
                    row.try_get::<String, _>("bound")?
                );
            } else {
                let mut defs = Vec::new();
                for col in columns(c, id).await? {
                    if col.try_get::<bool, _>("attislocal")? {
                        defs.push(column_definition(c, id, &col).await?);
                    }
                }
                definition = format!(
                    "CREATE {}TABLE {full} (\n    {}\n)",
                    if row.try_get::<String, _>("persistence")? == "u" {
                        "UNLOGGED "
                    } else {
                        ""
                    },
                    defs.join(",\n    ")
                );
                if !parents.is_empty() {
                    write!(
                        definition,
                        " INHERITS ({})",
                        parents
                            .iter()
                            .map(|(_, s, n)| format!("{}.{}", q(s), q(n)))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                    .ok();
                }
            }
            if let Some(key) = row.try_get::<Option<String>, _>("partition_key")? {
                write!(definition, " PARTITION BY {key}").ok();
            }
            let options: String = row.try_get("options")?;
            if !options.is_empty() {
                write!(definition, " WITH ({options})").ok();
            }
            write(out, &definition).await?;
            emitted.insert(id);
        }
        if next.len() == before {
            return Err(Error::Invalid(
                "Cannot order inherited/partitioned tables".into(),
            ));
        }
        waiting = next;
    }
    Ok(())
}
async fn data(c: &mut Pg, out: &mut Writer, schema: &str, table: &str, id: i64) -> Result<()> {
    let cols = columns(c, id).await?;
    let names = cols
        .iter()
        .filter(|r| {
            r.try_get::<String, _>("generated")
                .is_ok_and(|s| s.is_empty())
        })
        .map(|r| r.try_get::<String, _>("name").map(|s| q(&s)))
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let target = format!("{}.{}", q(schema), q(table));
    // Server text output plus quote_nullable preserves arrays, domains, enums,
    // bytea, numeric precision and extension types without a lossy client decoder.
    let expression = if names.is_empty() {
        text(&format!("INSERT INTO {target} DEFAULT VALUES;"))
    } else {
        format!(
            "{} || concat_ws(', ', {}) || ');'",
            text(&format!(
                "INSERT INTO {target} ({}) OVERRIDING SYSTEM VALUE VALUES (",
                names.join(", ")
            )),
            names
                .iter()
                .map(|n| format!("quote_nullable({n})"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    let query = format!("SELECT {expression} FROM ONLY {target}");
    let mut stream = sqlx::query_scalar::<_, String>(&query).fetch(&mut *c);
    while let Some(line) = stream.try_next().await? {
        out.write_all(line.as_bytes()).await?;
        out.write_all(b"\n").await?;
    }
    Ok(())
}
async fn views(
    c: &mut Pg,
    out: &mut Writer,
    schema: &str,
    relations: &[sqlx::postgres::PgRow],
    refresh: bool,
) -> Result<()> {
    let mut waiting = relations
        .iter()
        .filter(|r| {
            r.try_get::<String, _>("kind")
                .is_ok_and(|k| ["v", "m"].contains(&k.as_str()))
        })
        .collect::<Vec<_>>();
    let ids = waiting
        .iter()
        .map(|r| r.try_get::<i64, _>("id"))
        .collect::<std::result::Result<std::collections::HashSet<_>, _>>()?;
    let mut emitted = std::collections::HashSet::<i64>::new();
    while !waiting.is_empty() {
        let before = waiting.len();
        let mut next = Vec::new();
        for row in waiting {
            let id: i64 = row.try_get("id")?;
            let dependencies:Vec<i64>=sqlx::query_scalar("SELECT DISTINCT d.refobjid::bigint FROM pg_rewrite r JOIN pg_depend d ON d.classid='pg_rewrite'::regclass AND d.objid=r.oid WHERE r.ev_class=$1::bigint::oid AND d.refclassid='pg_class'::regclass AND d.refobjid<>r.ev_class").bind(id).fetch_all(&mut *c).await?;
            if dependencies
                .iter()
                .any(|d| ids.contains(d) && !emitted.contains(d))
            {
                next.push(row);
                continue;
            }
            let definition: String =
                sqlx::query_scalar("SELECT pg_get_viewdef($1::bigint::oid,true)")
                    .bind(id)
                    .fetch_one(&mut *c)
                    .await?;
            let materialized = row.try_get::<String, _>("kind")? == "m";
            let full = format!("{}.{}", q(schema), q(&row.try_get::<String, _>("name")?));
            let options: String = row.try_get("options")?;
            write(
                out,
                &format!(
                    "CREATE {}VIEW {full}{} AS\n{}{}",
                    if materialized { "MATERIALIZED " } else { "" },
                    if options.is_empty() {
                        String::new()
                    } else {
                        format!(" WITH ({options})")
                    },
                    definition.trim_end_matches([';', '\n']),
                    if materialized { " WITH NO DATA" } else { "" }
                ),
            )
            .await?;
            if materialized && refresh {
                write(out, &format!("REFRESH MATERIALIZED VIEW {full}")).await?;
            }
            emitted.insert(id);
        }
        if next.len() == before {
            return Err(Error::Invalid(
                "Cyclic view dependencies cannot be exported".into(),
            ));
        }
        waiting = next;
    }
    Ok(())
}
async fn post_data(
    c: &mut Pg,
    out: &mut Writer,
    schema: &str,
    relations: &[sqlx::postgres::PgRow],
) -> Result<()> {
    // Constraints follow all tables and rows, so cycles and alphabetical FK order work.
    for row in sqlx::query("SELECT c.relname::text AS name,k.conname::text,pg_get_constraintdef(k.oid,true) AS definition FROM pg_constraint k JOIN pg_class c ON c.oid=k.conrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND k.conislocal AND k.conparentid=0 AND k.contype<>'n' AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='e') ORDER BY CASE WHEN k.contype='f' THEN 1 ELSE 0 END,k.oid").bind(schema).fetch_all(&mut *c).await? {
        write(out,&format!("ALTER TABLE {}.{} ADD CONSTRAINT {} {}",q(schema),q(&row.try_get::<String,_>("name")?),q(&row.try_get::<String,_>("conname")?),row.try_get::<String,_>("definition")?)).await?;
    }
    for definition in sqlx::query_scalar::<_,String>("SELECT pg_get_indexdef(i.indexrelid) FROM pg_index i JOIN pg_class t ON t.oid=i.indrelid JOIN pg_namespace n ON n.oid=t.relnamespace WHERE n.nspname=$1 AND NOT EXISTS(SELECT 1 FROM pg_constraint c WHERE c.conindid=i.indexrelid) AND NOT EXISTS(SELECT 1 FROM pg_inherits h WHERE h.inhrelid=i.indexrelid) AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=t.oid AND d.deptype='e') ORDER BY i.indexrelid").bind(schema).fetch_all(&mut *c).await? {write(out,&definition).await?;}
    for row in sqlx::query("SELECT t.tgname::text,c.relname::text,pg_get_triggerdef(t.oid,true) AS definition,t.tgenabled::text FROM pg_trigger t JOIN pg_class c ON c.oid=t.tgrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND NOT t.tgisinternal AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_trigger'::regclass AND d.objid=t.oid AND d.deptype='e') ORDER BY t.oid").bind(schema).fetch_all(&mut *c).await? {
        write(out,&row.try_get::<String,_>("definition")?).await?;
        let enabled:String=row.try_get("tgenabled")?;let action=match enabled.as_str(){"D"=>"DISABLE", "R"=>"ENABLE REPLICA", "A"=>"ENABLE ALWAYS",_=>"ENABLE"};
        write(out,&format!("ALTER TABLE {}.{} {action} TRIGGER {}",q(schema),q(&row.try_get::<String,_>("relname")?),q(&row.try_get::<String,_>("tgname")?))).await?;
    }
    for definition in sqlx::query_scalar::<_,String>("SELECT pg_get_ruledef(r.oid,true) FROM pg_rewrite r JOIN pg_class c ON c.oid=r.ev_class JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND r.rulename<>'_RETURN' ORDER BY r.oid").bind(schema).fetch_all(&mut *c).await? {write(out,definition.trim_end_matches(';')).await?;}
    for row in sqlx::query("SELECT c.relname::text,p.polname::text,p.polpermissive,p.polcmd::text,ARRAY(SELECT CASE WHEN role_id=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(role_id)) END FROM unnest(p.polroles) role_id) AS roles,pg_get_expr(p.polqual,p.polrelid) AS using_expr,pg_get_expr(p.polwithcheck,p.polrelid) AS check_expr FROM pg_policy p JOIN pg_class c ON c.oid=p.polrelid JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 ORDER BY p.oid").bind(schema).fetch_all(&mut *c).await? {
        let command=match row.try_get::<String,_>("polcmd")?.as_str(){"r"=>"SELECT","a"=>"INSERT","w"=>"UPDATE","d"=>"DELETE",_=>"ALL"};
        let mut sql=format!("CREATE POLICY {} ON {}.{} AS {} FOR {command} TO {}",q(&row.try_get::<String,_>("polname")?),q(schema),q(&row.try_get::<String,_>("relname")?),if row.try_get::<bool,_>("polpermissive")?{"PERMISSIVE"}else{"RESTRICTIVE"},row.try_get::<Vec<String>,_>("roles")?.join(", "));
        if let Some(expr)=row.try_get::<Option<String>,_>("using_expr")?{write!(sql, " USING ({expr})").ok();}
        if let Some(expr)=row.try_get::<Option<String>,_>("check_expr")?{write!(sql, " WITH CHECK ({expr})").ok();}write(out,&sql).await?;
    }
    for definition in sqlx::query_scalar::<_,String>("SELECT pg_get_statisticsobjdef(s.oid) FROM pg_statistic_ext s JOIN pg_namespace n ON n.oid=s.stxnamespace WHERE n.nspname=$1 ORDER BY s.oid").bind(schema).fetch_all(&mut *c).await? {write(out,&definition).await?;}
    for row in relations {
        let full = format!("{}.{}", q(schema), q(&row.try_get::<String, _>("name")?));
        if row.try_get::<bool, _>("relrowsecurity")? {
            write(
                out,
                &format!("ALTER TABLE {full} ENABLE ROW LEVEL SECURITY"),
            )
            .await?;
        }
        if row.try_get::<bool, _>("relforcerowsecurity")? {
            write(out, &format!("ALTER TABLE {full} FORCE ROW LEVEL SECURITY")).await?;
        }
    }
    Ok(())
}
async fn ownership(
    c: &mut Pg,
    out: &mut Writer,
    schema: &str,
    sequences: &[sqlx::postgres::PgRow],
) -> Result<()> {
    for row in sequences {
        if row.try_get::<bool, _>("identity")? {
            continue;
        }
        let id: i64 = row.try_get("id")?;
        for (ns,table,column) in sqlx::query_as::<_,(String,String,String)>("SELECT n.nspname::text,t.relname::text,a.attname::text FROM pg_depend d JOIN pg_class t ON t.oid=d.refobjid JOIN pg_namespace n ON n.oid=t.relnamespace JOIN pg_attribute a ON a.attrelid=t.oid AND a.attnum=d.refobjsubid WHERE d.classid='pg_class'::regclass AND d.objid=$1::bigint::oid AND d.deptype='a'").bind(id).fetch_all(&mut *c).await?{write(out,&format!("ALTER SEQUENCE {}.{} OWNED BY {}.{}.{}",q(schema),q(&row.try_get::<String,_>("name")?),q(&ns),q(&table),q(&column))).await?;}
    }
    metadata::emit(c, out, schema).await?;
    let row=sqlx::query("SELECT pg_get_userbyid(nspowner) AS owner,obj_description(oid,'pg_namespace') AS comment FROM pg_namespace WHERE nspname=$1").bind(schema).fetch_one(&mut *c).await?;
    if let Some(comment) = row.try_get::<Option<String>, _>("comment")? {
        write(
            out,
            &format!("COMMENT ON SCHEMA {} IS {}", q(schema), text(&comment)),
        )
        .await?;
    }
    write(
        out,
        &format!(
            "ALTER SCHEMA {} OWNER TO {}",
            q(schema),
            q(&row.try_get::<String, _>("owner")?)
        ),
    )
    .await?;
    for row in sqlx::query("SELECT CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END AS role,a.privilege_type,a.is_grantable FROM pg_namespace n CROSS JOIN LATERAL aclexplode(n.nspacl) a WHERE n.nspname=$1 ORDER BY 1,2").bind(schema).fetch_all(&mut *c).await?{write(out,&format!("GRANT {} ON SCHEMA {} TO {}{}",row.try_get::<String,_>("privilege_type")?,q(schema),row.try_get::<String,_>("role")?,if row.try_get::<bool,_>("is_grantable")?{" WITH GRANT OPTION"}else{""})).await?;}
    Ok(())
}

async fn collations(c: &mut Pg, out: &mut Writer, schema: &str) -> Result<()> {
    for row in sqlx::query("SELECT p.collname::text AS name,p.collprovider::text AS provider,p.collcollate::text AS collate,p.collctype::text AS ctype,COALESCE(to_jsonb(p)->>'colliculocale',to_jsonb(p)->>'colllocale') AS locale,p.collisdeterministic,p.collversion::text AS version FROM pg_collation p JOIN pg_namespace n ON n.oid=p.collnamespace WHERE n.nspname=$1 AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_collation'::regclass AND d.objid=p.oid AND d.deptype='e') ORDER BY p.oid").bind(schema).fetch_all(&mut *c).await? {
        let provider:String=row.try_get("provider")?;let mut properties=match provider.as_str(){
            "c"=>format!("provider=libc,lc_collate={},lc_ctype={}",text(&row.try_get::<String,_>("collate")?),text(&row.try_get::<String,_>("ctype")?)),
            "i"=>format!("provider=icu,locale={}",text(&row.try_get::<String,_>("locale")?)),
            _=>return Err(Error::Invalid(format!("Unsupported collation provider {provider}"))),
        };
        write!(properties,",deterministic={}",row.try_get::<bool,_>("collisdeterministic")?).ok();
        if let Some(version)=row.try_get::<Option<String>,_>("version")?{write!(properties,",version={}",text(&version)).ok();}
        write(out,&format!("CREATE COLLATION {}.{} ({properties})",q(schema),q(&row.try_get::<String,_>("name")?))).await?;
    }
    Ok(())
}
async fn check_supported(c: &mut Pg, schema: &str) -> Result<()> {
    // Never return a deceptively complete DDL when a catalog category is not
    // implemented. Extension members are covered by CREATE EXTENSION instead.
    for (catalog, namespace, name) in [
        ("pg_operator", "oprnamespace", "oprname"),
        ("pg_opclass", "opcnamespace", "opcname"),
        ("pg_opfamily", "opfnamespace", "opfname"),
        ("pg_conversion", "connamespace", "conname"),
        ("pg_ts_config", "cfgnamespace", "cfgname"),
        ("pg_ts_dict", "dictnamespace", "dictname"),
        ("pg_ts_parser", "prsnamespace", "prsname"),
        ("pg_ts_template", "tmplnamespace", "tmplname"),
    ] {
        let sql = format!(
            "SELECT o.{name}::text FROM {catalog} o JOIN pg_namespace n ON n.oid=o.{namespace} WHERE n.nspname=$1 AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='{catalog}'::regclass AND d.objid=o.oid AND d.deptype='e') LIMIT 1"
        );
        if let Some(name) = sqlx::query_scalar::<_, String>(&sql)
            .bind(schema)
            .fetch_optional(&mut *c)
            .await?
        {
            return Err(Error::Invalid(format!(
                "Cannot export {catalog} object {schema}.{name} losslessly yet"
            )));
        }
    }
    Ok(())
}
