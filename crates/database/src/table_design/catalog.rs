//! Native table catalogs; snapshots never reconstruct unchanged advanced clauses.
use super::{Column, Definition, Element, Properties, Result, TableGrant, invalid, sql};
use crate::{
    Connection, DbDriverType,
    connection::{Pool, quote_identifier},
};
use sqlx::{Column as _, Row};
impl Connection {
    /// Load an editable table snapshot without changing persistent data.
    /// # Errors
    /// Returns catalog/permission errors or unsupported relation kinds.
    pub async fn table_definition(&self, schema: &str, table: &str) -> Result<Definition> {
        match &self.pool {
            Pool::ClickHouse(_) => Err(invalid("Use ClickHouse DDL in the SQL console")),
            Pool::Sqlite(p) => {
                let mut c = p.acquire().await?;
                c.lock_handle().await?.remove_progress_handler();
                sqlite(&mut c, schema, table).await
            }
            Pool::Mysql(p) => {
                let mut c = p.acquire().await?;
                c.close_on_drop();
                sqlx::raw_sql("SET SQL_MODE='STRICT_ALL_TABLES,NO_AUTO_VALUE_ON_ZERO,NO_BACKSLASH_ESCAPES'; SET sql_quote_show_create=1;").execute(&mut *c).await?;
                mysql(&mut c, schema, table).await
            }
            Pool::Postgres(p) => {
                let mut c = p.acquire().await?;
                c.close_on_drop();
                sqlx::raw_sql(&format!(
                    "SET search_path TO {},pg_catalog",
                    quote_identifier(self.driver, schema)
                ))
                .execute(&mut *c)
                .await?;
                postgres(&mut c, schema, table).await
            }
        }
    }
}
pub(super) async fn sqlite(
    c: &mut sqlx::SqliteConnection,
    schema: &str,
    table: &str,
) -> Result<Definition> {
    let q = |s: &str| quote_identifier(DbDriverType::Sqlite, s);
    let source: String = sqlx::query_scalar(&format!(
        "SELECT sql FROM {}.sqlite_master WHERE type='table' AND name=?",
        q(schema)
    ))
    .bind(table)
    .fetch_one(&mut *c)
    .await?;
    if source.to_ascii_uppercase().starts_with("CREATE VIRTUAL") {
        return Err(invalid(
            "Virtual tables must be modified through their SQLite module",
        ));
    }
    let (body, suffix) = sql::parentheses(&source)?;
    let mut def = Definition {
        driver: DbDriverType::Sqlite,
        schema: schema.into(),
        name: table.into(),
        source_sql: source.clone(),
        strict: suffix.to_ascii_uppercase().contains("STRICT"),
        without_rowid: suffix.to_ascii_uppercase().contains("WITHOUT ROWID"),
        ..Definition::default()
    };
    parse_body(&mut def, &body)?;
    for row in sqlx::query(&format!("SELECT type,name,sql FROM {}.sqlite_master WHERE tbl_name=? AND type IN ('index','trigger') AND sql IS NOT NULL ORDER BY type,name",q(schema))).bind(table).fetch_all(&mut *c).await? {
        let name:String=row.try_get("name")?;let definition:String=row.try_get("sql")?;let id=def.next_id();
        def.elements.push(if row.try_get::<String,_>("type")?=="index"{sql::index(id,&definition)?}else{sql::trigger(id,name,&definition)?});
    }
    def.sqlite_foreign_keys = sqlx::query_scalar::<_, i64>("PRAGMA foreign_keys")
        .fetch_one(&mut *c)
        .await?
        != 0;
    def.sqlite_legacy_alter = sqlx::query_scalar::<_, i64>("PRAGMA legacy_alter_table")
        .fetch_one(&mut *c)
        .await?
        != 0;
    def.catalog_revision = sqlx::query_scalar(&format!("PRAGMA {}.schema_version", q(schema)))
        .fetch_one(c)
        .await?;
    Ok(def)
}
fn parse_body(def: &mut Definition, body: &str) -> Result<()> {
    for clause in sql::parts(body)? {
        let ts = sql::tokens(&clause)?;
        let first = ts
            .first()
            .ok_or_else(|| invalid("Empty CREATE TABLE clause"))?;
        let id = def.next_id();
        let e = if [
            "CONSTRAINT",
            "PRIMARY",
            "UNIQUE",
            "FOREIGN",
            "CHECK",
            "EXCLUDE",
        ]
        .iter()
        .any(|s| first.is(s))
        {
            sql::key(id, &clause)?
        } else if ["KEY", "INDEX", "FULLTEXT", "SPATIAL"]
            .iter()
            .any(|s| first.is(s))
        {
            sql::index(id, &clause)?
        } else {
            sql::column(id, &clause)?
        };
        def.elements.push(e);
    }
    Ok(())
}
async fn mysql(c: &mut sqlx::MySqlConnection, schema: &str, table: &str) -> Result<Definition> {
    let q = |s: &str| quote_identifier(DbDriverType::Mysql, s);
    let row = sqlx::query(&format!("SHOW CREATE TABLE {}.{}", q(schema), q(table)))
        .fetch_one(&mut *c)
        .await?;
    let source: String = row.try_get_unchecked(1)?;
    let (body, suffix) = sql::parentheses(&source)?;
    let row=sqlx::query("SELECT CAST(ENGINE AS CHAR) AS engine,CAST(TABLE_COLLATION AS CHAR) AS collation,CAST(ROW_FORMAT AS CHAR) AS row_format,CAST(COALESCE(AUTO_INCREMENT,0) AS CHAR) AS counter,CAST(TABLE_COMMENT AS CHAR) AS comment FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=? AND TABLE_TYPE='BASE TABLE'").bind(schema).bind(table).fetch_one(&mut *c).await?;
    let mut def = Definition {
        driver: DbDriverType::Mysql,
        schema: schema.into(),
        name: table.into(),
        comment: row.try_get("comment")?,
        engine: row.try_get("engine")?,
        collation: row.try_get("collation")?,
        row_format: row.try_get("row_format")?,
        auto_increment: row.try_get("counter")?,
        source_sql: source,
        ..Definition::default()
    };
    def.server_version = sqlx::query_scalar("SELECT VERSION()")
        .fetch_one(&mut *c)
        .await?;
    if let Some(at) = suffix.to_ascii_uppercase().find("PARTITION BY") {
        def.partition_sql = suffix[at..].into();
    }
    parse_body(&mut def, &body)?;
    for name in sqlx::query_scalar::<_,String>("SELECT CAST(TRIGGER_NAME AS CHAR) FROM information_schema.TRIGGERS WHERE TRIGGER_SCHEMA=? AND EVENT_OBJECT_TABLE=? ORDER BY TRIGGER_NAME").bind(schema).bind(table).fetch_all(&mut *c).await?{
        let row=sqlx::query(&format!("SHOW CREATE TRIGGER {}.{}",q(schema),q(&name))).fetch_one(&mut *c).await?;
        let field=|name:&str|row.columns().iter().position(|c|c.name().eq_ignore_ascii_case(name)).ok_or_else(||invalid(format!("Missing trigger metadata {name}")));
        let source:String=row.try_get_unchecked(field("SQL Original Statement")?)?;
        let mode:String=row.try_get_unchecked(field("sql_mode")?)?;let mut e=sql::mysql_trigger(def.next_id(),name,&source,&mode)?;
        if let Properties::Trigger(t)=&mut e.data{t.sql_mode=mode;t.preserve_sql_mode=true;}
        def.elements.push(e);
    }
    for row in sqlx::query("SELECT CAST(GRANTEE AS CHAR) AS account,CAST(PRIVILEGE_TYPE AS CHAR) AS privilege,CAST(IS_GRANTABLE AS CHAR) AS grantable FROM information_schema.TABLE_PRIVILEGES WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY GRANTEE,PRIVILEGE_TYPE").bind(schema).bind(table).fetch_all(c).await? {
        let account:String=row.try_get("account")?;let (user,host)=account.strip_prefix('\'').and_then(|s|s.strip_suffix('\'')).and_then(|s|s.rsplit_once("'@'")).ok_or_else(||invalid("Invalid MySQL account metadata"))?;
        def.grants.push(TableGrant{user:user.replace("''","'"),host:host.replace("''","'"),privilege:row.try_get("privilege")?,grant_option:row.try_get::<String,_>("grantable")?=="YES"});
    }
    Ok(def)
}
async fn postgres(c: &mut sqlx::PgConnection, schema: &str, table: &str) -> Result<Definition> {
    let row=sqlx::query("SELECT c.oid::bigint AS id,c.relpersistence::text,pg_get_userbyid(c.relowner) AS owner,COALESCE(obj_description(c.oid,'pg_class'),'') AS comment,COALESCE(t.spcname,'') AS tablespace,COALESCE(a.amname,'') AS method,COALESCE(array_to_string(c.reloptions,', '),'') AS options,c.relrowsecurity,c.relforcerowsecurity,COALESCE(pg_get_partkeydef(c.oid),pg_get_expr(c.relpartbound,c.oid,true),'') AS partition FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace LEFT JOIN pg_tablespace t ON t.oid=c.reltablespace LEFT JOIN pg_am a ON a.oid=c.relam WHERE n.nspname=$1 AND c.relname=$2 AND c.relkind IN ('r','p')").bind(schema).bind(table).fetch_one(&mut *c).await?;
    let id: i64 = row.try_get("id")?;
    let mut def = Definition {
        driver: DbDriverType::Postgres,
        schema: schema.into(),
        name: table.into(),
        comment: row.try_get("comment")?,
        owner: row.try_get("owner")?,
        tablespace: row.try_get("tablespace")?,
        access_method: row.try_get("method")?,
        storage_options: row.try_get("options")?,
        unlogged: row.try_get::<String, _>("relpersistence")? == "u",
        row_security: row.try_get("relrowsecurity")?,
        force_row_security: row.try_get("relforcerowsecurity")?,
        partition_sql: row.try_get("partition")?,
        ..Definition::default()
    };
    def.server_version = sqlx::query_scalar("SHOW server_version")
        .fetch_one(&mut *c)
        .await?;
    for row in sqlx::query("SELECT a.attname::text AS name,format_type(a.atttypid,a.atttypmod) AS kind,a.attnotnull,a.attidentity::text,a.attgenerated::text,COALESCE(pg_get_expr(d.adbin,d.adrelid),'') AS default_sql,COALESCE(col_description(a.attrelid,a.attnum),'') AS comment,CASE WHEN a.attcollation<>t.typcollation THEN quote_ident(n.nspname)||'.'||quote_ident(coll.collname) ELSE '' END AS collation,a.attnum FROM pg_attribute a JOIN pg_type t ON t.oid=a.atttypid LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum LEFT JOIN pg_collation coll ON coll.oid=a.attcollation LEFT JOIN pg_namespace n ON n.oid=coll.collnamespace WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum").bind(id).fetch_all(&mut *c).await? {
        let default:String=row.try_get("default_sql")?;let generated:String=row.try_get("attgenerated")?;let identity:String=row.try_get("attidentity")?;
        let identity_options=if identity.is_empty(){String::new()}else{sqlx::query_scalar::<_,String>("SELECT 'START WITH '||s.seqstart||' INCREMENT BY '||s.seqincrement||' MINVALUE '||s.seqmin||' MAXVALUE '||s.seqmax||' CACHE '||s.seqcache||CASE WHEN s.seqcycle THEN ' CYCLE' ELSE ' NO CYCLE' END FROM pg_sequence s JOIN pg_depend d ON d.classid='pg_class'::regclass AND d.objid=s.seqrelid WHERE d.refobjid=$1::bigint::oid AND d.refobjsubid=$2 AND d.deptype='i'").bind(id).bind(row.try_get::<i16,_>("attnum")?).fetch_one(&mut *c).await?};
        let identity_sequence=if identity.is_empty(){String::new()}else{sqlx::query_scalar::<_,String>("SELECT quote_ident(n.nspname)||'.'||quote_ident(s.relname) FROM pg_class s JOIN pg_namespace n ON n.oid=s.relnamespace JOIN pg_depend d ON d.classid='pg_class'::regclass AND d.objid=s.oid WHERE d.refobjid=$1::bigint::oid AND d.refobjsubid=$2 AND d.deptype='i'").bind(id).bind(row.try_get::<i16,_>("attnum")?).fetch_one(&mut *c).await?};
        def.elements.push(Element{id:def.next_id(),name:row.try_get("name")?,comment:row.try_get("comment")?,native_sql:String::new(),data:Properties::Column(Column{data_type:row.try_get("kind")?,not_null:row.try_get("attnotnull")?,default_sql:if generated.is_empty(){default.clone()}else{String::new()},generated_sql:if generated.is_empty(){String::new()}else{default},stored:generated!="v",identity:match identity.as_str(){"a"=>"ALWAYS","d"=>"BY DEFAULT",_=>""}.into(),identity_options,identity_sequence,collation:row.try_get("collation")?,..Column::default()})});
    }
    for row in sqlx::query("SELECT c.conname::text AS name,pg_get_constraintdef(c.oid,true) AS definition,COALESCE(obj_description(c.oid,'pg_constraint'),'') AS comment FROM pg_constraint c WHERE c.conrelid=$1::bigint::oid AND c.conislocal AND c.contype IN ('p','u','f','c','x') ORDER BY c.oid").bind(id).fetch_all(&mut *c).await? {
        let name:String=row.try_get("name")?;let source:String=row.try_get("definition")?;let mut e=sql::key(def.next_id(),&format!("CONSTRAINT {} {source}",quote_identifier(DbDriverType::Postgres,&name)))?;e.name=name;e.comment=row.try_get("comment")?;def.elements.push(e);
    }
    for row in sqlx::query("SELECT pg_get_indexdef(i.indexrelid) AS definition,COALESCE(obj_description(i.indexrelid,'pg_class'),'') AS comment FROM pg_index i WHERE i.indrelid=$1::bigint::oid AND NOT EXISTS(SELECT 1 FROM pg_constraint c WHERE c.conindid=i.indexrelid) ORDER BY i.indexrelid").bind(id).fetch_all(&mut *c).await? {let mut e=sql::index(def.next_id(),&row.try_get::<String,_>("definition")?)?;e.comment=row.try_get("comment")?;def.elements.push(e);}
    for row in sqlx::query("SELECT tgname::text AS name,pg_get_triggerdef(oid,true) AS definition,tgenabled::text AS enabled,COALESCE(obj_description(oid,'pg_trigger'),'') AS comment FROM pg_trigger WHERE tgrelid=$1::bigint::oid AND NOT tgisinternal ORDER BY oid").bind(id).fetch_all(&mut *c).await? {let mut e=sql::trigger(def.next_id(),row.try_get("name")?,&row.try_get::<String,_>("definition")?)?;e.comment=row.try_get("comment")?;if let Properties::Trigger(t)=&mut e.data {t.enabled=match row.try_get::<String,_>("enabled")?.as_str(){"D"=>"DISABLE","R"=>"ENABLE REPLICA","A"=>"ENABLE ALWAYS",_=>"ENABLE"}.into();}def.elements.push(e);}
    for row in sqlx::query("SELECT CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE pg_get_userbyid(a.grantee) END AS role,a.privilege_type,a.is_grantable FROM pg_class c CROSS JOIN LATERAL aclexplode(c.relacl) a WHERE c.oid=$1::bigint::oid ORDER BY 1,2").bind(id).fetch_all(c).await?{def.grants.push(TableGrant{user:row.try_get("role")?,host:String::new(),privilege:row.try_get("privilege_type")?,grant_option:row.try_get("is_grantable")?});}
    Ok(def)
}
