//! Owners, comments and explicit ACLs accompanying schema definitions.
use super::{Pg, Result, Writer, q, text, write};
use sqlx::Row;

pub(super) async fn emit(c: &mut Pg, out: &mut Writer, schema: &str) -> Result<()> {
    relations(c, out, schema).await?;
    routines(c, out, schema).await?;
    types(c, out, schema).await?;
    defaults(c, out, schema).await?;
    Ok(())
}
async fn relations(c: &mut Pg, out: &mut Writer, schema: &str) -> Result<()> {
    let rows=sqlx::query("SELECT c.oid::bigint AS id,c.relname::text AS name,c.relkind::text AS kind,pg_get_userbyid(c.relowner) AS owner,obj_description(c.oid,'pg_class') AS comment,c.relacl IS NOT NULL AS has_acl,EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='i') AS identity FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relkind IN ('r','p','v','m','S') AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_class'::regclass AND d.objid=c.oid AND d.deptype='e') ORDER BY CASE WHEN c.relkind='S' THEN 1 ELSE 0 END,c.oid").bind(schema).fetch_all(&mut *c).await?;
    for row in rows {
        let id: i64 = row.try_get("id")?;
        let name: String = row.try_get("name")?;
        let kind: String = row.try_get("kind")?;
        let object = match kind.as_str() {
            "S" => "SEQUENCE",
            "v" => "VIEW",
            "m" => "MATERIALIZED VIEW",
            _ => "TABLE",
        };
        let full = format!("{}.{}", q(schema), q(&name));
        if let Some(comment) = row.try_get::<Option<String>, _>("comment")? {
            write(
                out,
                &format!("COMMENT ON {object} {full} IS {}", text(&comment)),
            )
            .await?;
        }
        if object != "SEQUENCE" {
            for (column,comment) in sqlx::query_as::<_,(String,String)>("SELECT a.attname::text,d.description FROM pg_attribute a JOIN pg_description d ON d.classoid='pg_class'::regclass AND d.objoid=a.attrelid AND d.objsubid=a.attnum WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum").bind(id).fetch_all(&mut *c).await?{write(out,&format!("COMMENT ON COLUMN {full}.{} IS {}",q(&column),text(&comment))).await?;}
        }
        if row.try_get::<bool, _>("has_acl")? {
            let privilege_object = if object == "SEQUENCE" {
                "SEQUENCE"
            } else {
                "TABLE"
            };
            write(
                out,
                &format!("REVOKE ALL ON {privilege_object} {full} FROM PUBLIC"),
            )
            .await?;
            for acl in sqlx::query("SELECT CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END AS role,a.privilege_type,a.is_grantable FROM pg_class c CROSS JOIN LATERAL aclexplode(c.relacl) a WHERE c.oid=$1::bigint::oid ORDER BY 1,2").bind(id).fetch_all(&mut *c).await? {grant(out,&acl,&format!("{privilege_object} {full}")).await?;}
        }
        for acl in sqlx::query("SELECT a.attname::text AS column,CASE WHEN g.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(g.grantee)) END AS role,g.privilege_type,g.is_grantable FROM pg_attribute a CROSS JOIN LATERAL aclexplode(a.attacl) g WHERE a.attrelid=$1::bigint::oid AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum,2,3").bind(id).fetch_all(&mut *c).await? {
            write(out,&format!("GRANT {} ({}) ON TABLE {full} TO {}{}",acl.try_get::<String,_>("privilege_type")?,q(&acl.try_get::<String,_>("column")?),acl.try_get::<String,_>("role")?,if acl.try_get::<bool,_>("is_grantable")?{" WITH GRANT OPTION"}else{""})).await?;
        }
        if !row.try_get::<bool, _>("identity")? {
            write(
                out,
                &format!(
                    "ALTER {object} {full} OWNER TO {}",
                    q(&row.try_get::<String, _>("owner")?)
                ),
            )
            .await?;
        }
    }
    Ok(())
}
async fn routines(c: &mut Pg, out: &mut Writer, schema: &str) -> Result<()> {
    for row in sqlx::query("SELECT p.oid::bigint AS id,p.proname::text AS name,p.prokind::text AS kind,pg_get_function_identity_arguments(p.oid) AS arguments,pg_get_userbyid(p.proowner) AS owner,obj_description(p.oid,'pg_proc') AS comment,p.proacl IS NOT NULL AS has_acl FROM pg_proc p JOIN pg_namespace n ON n.oid=p.pronamespace WHERE n.nspname=$1 AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_proc'::regclass AND d.objid=p.oid AND d.deptype='e') ORDER BY p.oid").bind(schema).fetch_all(&mut *c).await? {
        let kind=if row.try_get::<String,_>("kind")?=="p"{"PROCEDURE"}else{"FUNCTION"};let full=format!("{}.{}({})",q(schema),q(&row.try_get::<String,_>("name")?),row.try_get::<String,_>("arguments")?);
        if let Some(comment)=row.try_get::<Option<String>,_>("comment")?{write(out,&format!("COMMENT ON {kind} {full} IS {}",text(&comment))).await?;}
        if row.try_get::<bool,_>("has_acl")?{write(out,&format!("REVOKE ALL ON {kind} {full} FROM PUBLIC")).await?;for acl in sqlx::query("SELECT CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END AS role,a.privilege_type,a.is_grantable FROM pg_proc p CROSS JOIN LATERAL aclexplode(p.proacl) a WHERE p.oid=$1::bigint::oid ORDER BY 1,2").bind(row.try_get::<i64,_>("id")?).fetch_all(&mut *c).await?{grant(out,&acl,&format!("{kind} {full}")).await?;}}
        write(out,&format!("ALTER {kind} {full} OWNER TO {}",q(&row.try_get::<String,_>("owner")?))).await?;
    }
    Ok(())
}
async fn types(c: &mut Pg, out: &mut Writer, schema: &str) -> Result<()> {
    for row in sqlx::query("SELECT t.oid::bigint AS id,t.typname::text AS name,t.typtype::text AS kind,pg_get_userbyid(t.typowner) AS owner,obj_description(t.oid,'pg_type') AS comment,t.typacl IS NOT NULL AS has_acl FROM pg_type t JOIN pg_namespace n ON n.oid=t.typnamespace LEFT JOIN pg_class r ON r.oid=t.typrelid WHERE n.nspname=$1 AND t.typelem=0 AND (t.typrelid=0 OR r.relkind='c') AND NOT EXISTS(SELECT 1 FROM pg_depend d WHERE d.classid='pg_type'::regclass AND d.objid=t.oid AND d.deptype='e') ORDER BY t.oid").bind(schema).fetch_all(&mut *c).await? {
        let full=format!("{}.{}",q(schema),q(&row.try_get::<String,_>("name")?));let kind=if row.try_get::<String,_>("kind")?=="d"{"DOMAIN"}else{"TYPE"};
        if let Some(comment)=row.try_get::<Option<String>,_>("comment")?{write(out,&format!("COMMENT ON {kind} {full} IS {}",text(&comment))).await?;}
        if row.try_get::<bool,_>("has_acl")?{write(out,&format!("REVOKE ALL ON TYPE {full} FROM PUBLIC")).await?;for acl in sqlx::query("SELECT CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END AS role,a.privilege_type,a.is_grantable FROM pg_type t CROSS JOIN LATERAL aclexplode(t.typacl) a WHERE t.oid=$1::bigint::oid ORDER BY 1,2").bind(row.try_get::<i64,_>("id")?).fetch_all(&mut *c).await?{grant(out,&acl,&format!("TYPE {full}")).await?;}}
        write(out,&format!("ALTER {kind} {full} OWNER TO {}",q(&row.try_get::<String,_>("owner")?))).await?;
    }
    Ok(())
}
async fn defaults(c: &mut Pg, out: &mut Writer, schema: &str) -> Result<()> {
    for row in sqlx::query("SELECT pg_get_userbyid(d.defaclrole) AS owner,d.defaclobjtype::text AS kind,CASE WHEN a.grantee=0 THEN 'PUBLIC' ELSE quote_ident(pg_get_userbyid(a.grantee)) END AS role,a.privilege_type,a.is_grantable FROM pg_default_acl d JOIN pg_namespace n ON n.oid=d.defaclnamespace CROSS JOIN LATERAL aclexplode(d.defaclacl) a WHERE n.nspname=$1 ORDER BY d.oid,3,4").bind(schema).fetch_all(&mut *c).await? {
        let kind=match row.try_get::<String,_>("kind")?.as_str(){"r"=>"TABLES","S"=>"SEQUENCES","f"=>"FUNCTIONS","T"=>"TYPES",_=>return Err(crate::Error::Invalid("Unsupported default privilege object".into()))};
        write(out,&format!("ALTER DEFAULT PRIVILEGES FOR ROLE {} IN SCHEMA {} GRANT {} ON {kind} TO {}{}",q(&row.try_get::<String,_>("owner")?),q(schema),row.try_get::<String,_>("privilege_type")?,row.try_get::<String,_>("role")?,if row.try_get::<bool,_>("is_grantable")?{" WITH GRANT OPTION"}else{""})).await?;
    }
    Ok(())
}
async fn grant(out: &mut Writer, row: &sqlx::postgres::PgRow, object: &str) -> Result<()> {
    write(
        out,
        &format!(
            "GRANT {} ON {object} TO {}{}",
            row.try_get::<String, _>("privilege_type")?,
            row.try_get::<String, _>("role")?,
            if row.try_get::<bool, _>("is_grantable")? {
                " WITH GRANT OPTION"
            } else {
                ""
            }
        ),
    )
    .await
}
