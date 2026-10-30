//! Relation objects and DDL from native catalogs.
use crate::{
    Connection, Result,
    connection::{Pool, quote_identifier},
};
use sqlx::Row;

/// Named table object with its native definition or column list.
#[derive(Clone, Debug)]
pub struct TableObject {
    /// Object name.
    pub name: String,
    /// Columns, reference target or native definition.
    pub definition: String,
}
/// Objects displayed below a table in the explorer.
#[derive(Clone, Debug, Default)]
pub struct TableObjects {
    /// Primary/unique keys and check constraints.
    pub keys: Vec<TableObject>,
    /// Foreign key names and referenced columns.
    pub foreign_keys: Vec<TableObject>,
    /// Index names and definitions.
    pub indexes: Vec<TableObject>,
    /// Native or catalog-reconstructed relation DDL.
    pub ddl: String,
    /// Optional failure loading keys/indexes/DDL; column and data access remain usable.
    pub error: Option<String>,
}
impl Connection {
    // Names are bound values, or quoted identifiers in PRAGMA/SHOW syntax.
    #[allow(
        clippy::too_many_lines,
        reason = "Three native catalog dialects populate one table object contract"
    )]
    pub(crate) async fn table_objects(
        &self,
        schema: &str,
        table: &str,
        is_view: bool,
    ) -> Result<TableObjects> {
        let mut objects = TableObjects::default();
        let q = |s: &str| quote_identifier(self.driver, s);
        match &self.pool {
            Pool::Sqlite(pool) => {
                let rows = sqlx::query(&format!("SELECT sql FROM {}.sqlite_master WHERE tbl_name=? AND sql IS NOT NULL ORDER BY type DESC, name",q(schema))).bind(table).fetch_all(pool).await?;
                objects.ddl = rows
                    .iter()
                    .map(|r| r.try_get::<String, _>("sql"))
                    .collect::<std::result::Result<Vec<_>, _>>()?
                    .join(";\n\n")
                    + ";";
                let pk = sqlx::query(&format!("PRAGMA {}.table_info({})", q(schema), q(table)))
                    .fetch_all(pool)
                    .await?;
                let mut pk = pk
                    .iter()
                    .filter_map(|r| {
                        let order = r.try_get::<i64, _>("pk").ok()?;
                        (order > 0)
                            .then(|| (order, r.try_get::<String, _>("name").unwrap_or_default()))
                    })
                    .collect::<Vec<_>>();
                pk.sort_by_key(|(order, _)| *order);
                if !pk.is_empty() {
                    objects.keys.push(TableObject {
                        name: "PRIMARY".into(),
                        definition: format!(
                            "({})",
                            pk.iter().map(|(_, n)| q(n)).collect::<Vec<_>>().join(", ")
                        ),
                    });
                }
                for r in sqlx::query(&format!("PRAGMA {}.index_list({})", q(schema), q(table)))
                    .fetch_all(pool)
                    .await?
                {
                    let name: String = r.try_get("name")?;
                    let columns =
                        sqlx::query(&format!("PRAGMA {}.index_info({})", q(schema), q(&name)))
                            .fetch_all(pool)
                            .await?
                            .iter()
                            .map(|r| {
                                r.try_get::<Option<String>, _>("name")
                                    .map(|s| s.unwrap_or_else(|| "expression".into()))
                            })
                            .collect::<std::result::Result<Vec<_>, _>>()?
                            .join(", ");
                    let unique = r.try_get::<i64, _>("unique")? != 0;
                    let object = TableObject {
                        name,
                        definition: format!("({columns}){}", if unique { " UNIQUE" } else { "" }),
                    };
                    if unique {
                        objects.keys.push(object.clone());
                    }
                    objects.indexes.push(object);
                }
                for r in sqlx::query(&format!(
                    "PRAGMA {}.foreign_key_list({})",
                    q(schema),
                    q(table)
                ))
                .fetch_all(pool)
                .await?
                {
                    let id: i64 = r.try_get("id")?;
                    let from: String = r.try_get("from")?;
                    let target: String = r.try_get("table")?;
                    let to: Option<String> = r.try_get("to")?;
                    let definition = format!(
                        "({from}) → {target} ({})",
                        to.unwrap_or_else(|| "PRIMARY KEY".into())
                    );
                    let name = format!("fk_{id}");
                    if let Some(object) = objects.foreign_keys.iter_mut().find(|o| o.name == name) {
                        object.definition.push_str(", ");
                        object.definition.push_str(&definition);
                    } else {
                        objects.foreign_keys.push(TableObject { name, definition });
                    }
                }
            }
            Pool::Mysql(pool) => {
                let row = sqlx::query(&format!(
                    "SHOW CREATE {} {}.{}",
                    if is_view { "VIEW" } else { "TABLE" },
                    q(schema),
                    q(table)
                ))
                .fetch_one(pool)
                .await?;
                objects.ddl = format!("{};", row.try_get_unchecked::<String, _>(1)?);
                for r in sqlx::query("SELECT CAST(CONSTRAINT_NAME AS CHAR) AS name, CAST(CONSTRAINT_TYPE AS CHAR) AS kind FROM information_schema.TABLE_CONSTRAINTS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY CONSTRAINT_NAME").bind(schema).bind(table).fetch_all(pool).await? {
                    let name: String = r.try_get("name")?; let kind: String = r.try_get("kind")?;
                    let columns = sqlx::query("SELECT CAST(COLUMN_NAME AS CHAR) AS col, CAST(REFERENCED_TABLE_NAME AS CHAR) AS target, CAST(REFERENCED_COLUMN_NAME AS CHAR) AS dest FROM information_schema.KEY_COLUMN_USAGE WHERE TABLE_SCHEMA=? AND TABLE_NAME=? AND CONSTRAINT_NAME=? ORDER BY ORDINAL_POSITION").bind(schema).bind(table).bind(&name).fetch_all(pool).await?;
                    let mut parts = Vec::new();
                    for c in columns { let col: String = c.try_get("col")?; let target: Option<String> = c.try_get("target")?; let dest: Option<String> = c.try_get("dest")?; parts.push(format!("{col}{}",target.map_or(String::new(),|t|format!(" → {t} ({})",dest.unwrap_or_default())))); }
                    let object = TableObject { name, definition: format!("{kind} ({})",parts.join(", ")) };
                    if kind == "FOREIGN KEY" { objects.foreign_keys.push(object); } else { objects.keys.push(object); }
                }
                for r in sqlx::query("SELECT CAST(INDEX_NAME AS CHAR) AS name, CAST(COLUMN_NAME AS CHAR) AS col, CAST(NON_UNIQUE AS SIGNED) AS non_unique FROM information_schema.STATISTICS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY INDEX_NAME,SEQ_IN_INDEX").bind(schema).bind(table).fetch_all(pool).await? {
                    let name: String = r.try_get("name")?; let col: Option<String> = r.try_get("col")?;
                    let definition = format!("{}{}",col.unwrap_or_else(||"expression".into()),if r.try_get::<i64,_>("non_unique")? == 0 { " UNIQUE" } else { "" });
                    if let Some(index) = objects.indexes.iter_mut().find(|i|i.name==name) { index.definition.push_str(", "); index.definition.push_str(&definition); } else { objects.indexes.push(TableObject { name, definition }); }
                }
            }
            Pool::Postgres(pool) => {
                for r in sqlx::query("SELECT c.conname AS name, c.contype::text AS kind, pg_get_constraintdef(c.oid, true) AS definition FROM pg_constraint c JOIN pg_class t ON t.oid=c.conrelid JOIN pg_namespace n ON n.oid=t.relnamespace WHERE n.nspname=$1 AND t.relname=$2 ORDER BY c.conname").bind(schema).bind(table).fetch_all(pool).await? {
                    let object = TableObject { name: r.try_get("name")?, definition: r.try_get("definition")? };
                    if r.try_get::<String,_>("kind")? == "f" { objects.foreign_keys.push(object); } else { objects.keys.push(object); }
                }
                for r in sqlx::query("SELECT indexname, indexdef FROM pg_indexes WHERE schemaname=$1 AND tablename=$2 ORDER BY indexname").bind(schema).bind(table).fetch_all(pool).await? { objects.indexes.push(TableObject {name:r.try_get("indexname")?,definition:r.try_get("indexdef")?}); }
                if is_view {
                    let definition: String = sqlx::query_scalar("SELECT pg_get_viewdef(c.oid, true) FROM pg_class c JOIN pg_namespace n ON n.oid=c.relnamespace WHERE n.nspname=$1 AND c.relname=$2").bind(schema).bind(table).fetch_one(pool).await?;
                    objects.ddl =
                        format!("CREATE VIEW {}.{} AS\n{definition}", q(schema), q(table));
                } else {
                    let rows = sqlx::query("SELECT a.attname AS name, format_type(a.atttypid,a.atttypmod) AS kind, a.attnotnull AS required, a.attidentity::text AS identity, a.attgenerated::text AS generated, pg_get_expr(d.adbin,d.adrelid) AS default_value FROM pg_attribute a JOIN pg_class t ON t.oid=a.attrelid JOIN pg_namespace n ON n.oid=t.relnamespace LEFT JOIN pg_attrdef d ON d.adrelid=a.attrelid AND d.adnum=a.attnum WHERE n.nspname=$1 AND t.relname=$2 AND a.attnum>0 AND NOT a.attisdropped ORDER BY a.attnum").bind(schema).bind(table).fetch_all(pool).await?;
                    let mut definitions = Vec::new();
                    for r in rows {
                        let name: String = r.try_get("name")?;
                        let kind: String = r.try_get("kind")?;
                        let identity: String = r.try_get("identity")?;
                        let generated: String = r.try_get("generated")?;
                        let default: Option<String> = r.try_get("default_value")?;
                        let extra = if identity == "a" {
                            " GENERATED ALWAYS AS IDENTITY".into()
                        } else if identity == "d" {
                            " GENERATED BY DEFAULT AS IDENTITY".into()
                        } else if !generated.is_empty() {
                            format!(
                                " GENERATED ALWAYS AS ({}) STORED",
                                default.unwrap_or_default()
                            )
                        } else {
                            default.map_or(String::new(), |v| format!(" DEFAULT {v}"))
                        };
                        definitions.push(format!(
                            "    {} {kind}{extra}{}",
                            q(&name),
                            if r.try_get::<bool, _>("required")? {
                                " NOT NULL"
                            } else {
                                ""
                            }
                        ));
                    }
                    for c in objects.keys.iter().chain(&objects.foreign_keys) {
                        definitions.push(format!("    CONSTRAINT {} {}", q(&c.name), c.definition));
                    }
                    objects.ddl = format!(
                        "CREATE TABLE {}.{} (\n{}\n);",
                        q(schema),
                        q(table),
                        definitions.join(",\n")
                    );
                    for i in &objects.indexes {
                        if !objects.keys.iter().any(|k| k.name == i.name) {
                            objects.ddl.push('\n');
                            objects.ddl.push_str(&i.definition);
                            objects.ddl.push(';');
                        }
                    }
                }
            }
        }
        Ok(objects)
    }
}
