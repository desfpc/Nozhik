//! Catalog metadata using bound names and dialect-specific system catalogs.
use crate::{
    Connection, DbDriverType, Result,
    connection::{Pool, quote_identifier},
};
use sqlx::Row;

/// Metadata tree for one connected database (MySQL catalogs may include others).
#[derive(Clone, Debug, Default)]
pub struct DatabaseSchemaTree {
    /// Schema/database namespaces.
    pub schemas: Vec<SchemaInfo>,
}
/// One namespace, containing tables and views.
#[derive(Clone, Debug)]
pub struct SchemaInfo {
    /// SQL namespace identifier.
    pub name: String,
    /// Relations in this namespace.
    pub tables: Vec<TableInfo>,
}
/// Table or view with ordered columns.
#[derive(Clone, Debug)]
pub struct TableInfo {
    /// SQL relation identifier.
    pub name: String,
    /// True for views.
    pub is_view: bool,
    /// Ordered column metadata.
    pub columns: Vec<SchemaColumn>,
}
/// Schema column, distinct from a query result column alias.
#[derive(Clone, Debug)]
pub struct SchemaColumn {
    /// Column identifier.
    pub name: String,
    /// Declared SQL type.
    pub data_type: String,
    /// Whether SQL NULL is allowed.
    pub nullable: bool,
    /// Member of a single or composite primary key.
    pub is_primary_key: bool,
}
impl Connection {
    /// Read visible catalog metadata. SQLite includes attached namespaces.
    /// # Errors
    /// Returns driver errors or malformed catalog values.
    #[allow(
        clippy::too_many_lines,
        reason = "Three catalog dialects share the same metadata contract"
    )]
    pub async fn introspect(&self) -> Result<DatabaseSchemaTree> {
        let mut tree = DatabaseSchemaTree::default();
        match &self.pool {
            Pool::Sqlite(pool) => {
                for row in sqlx::query("PRAGMA database_list").fetch_all(pool).await? {
                    let name: String = row.try_get("name")?;
                    let quoted = quote_identifier(DbDriverType::Sqlite, &name);
                    let tables = sqlx::query(&format!("SELECT name, type FROM {quoted}.sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name")).fetch_all(pool).await?;
                    let mut schema = SchemaInfo {
                        name,
                        tables: Vec::new(),
                    };
                    for table in tables {
                        let name: String = table.try_get("name")?;
                        let mut info = TableInfo {
                            name: name.clone(),
                            is_view: table.try_get::<String, _>("type")? == "view",
                            columns: Vec::new(),
                        };
                        let columns = sqlx::query(&format!(
                            "PRAGMA {quoted}.table_info({})",
                            quote_identifier(DbDriverType::Sqlite, &name)
                        ))
                        .fetch_all(pool)
                        .await?;
                        let single_pk = columns
                            .iter()
                            .filter(|c| c.try_get::<i64, _>("pk").is_ok_and(|pk| pk > 0))
                            .count()
                            == 1;
                        for c in columns {
                            let pk = c.try_get::<i64, _>("pk")? > 0;
                            let data_type: String = c.try_get("type")?;
                            // Legacy rowid tables permit NULL in non-integer PKs.
                            let rowid_pk =
                                pk && single_pk && data_type.eq_ignore_ascii_case("INTEGER");
                            info.columns.push(SchemaColumn {
                                name: c.try_get("name")?,
                                data_type,
                                nullable: c.try_get::<i64, _>("notnull")? == 0 && !rowid_pk,
                                is_primary_key: pk,
                            });
                        }
                        schema.tables.push(info);
                    }
                    tree.schemas.push(schema);
                }
            }
            Pool::Postgres(pool) => {
                let schemas = sqlx::query("SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('pg_catalog','information_schema') AND schema_name NOT LIKE 'pg_toast%' ORDER BY schema_name").fetch_all(pool).await?;
                for s in schemas {
                    let name: String = s.try_get("schema_name")?;
                    let tables = sqlx::query("SELECT table_name, table_type FROM information_schema.tables WHERE table_schema = $1 ORDER BY table_name").bind(&name).fetch_all(pool).await?;
                    let mut schema = SchemaInfo {
                        name,
                        tables: Vec::new(),
                    };
                    for t in tables {
                        let name: String = t.try_get("table_name")?;
                        let rows = sqlx::query("SELECT c.column_name, c.data_type, c.is_nullable, EXISTS (SELECT 1 FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage k ON tc.constraint_catalog=k.constraint_catalog AND tc.constraint_schema=k.constraint_schema AND tc.constraint_name=k.constraint_name AND tc.table_name=k.table_name WHERE tc.constraint_type='PRIMARY KEY' AND k.table_schema=c.table_schema AND k.table_name=c.table_name AND k.column_name=c.column_name) AS pk FROM information_schema.columns c WHERE c.table_schema=$1 AND c.table_name=$2 ORDER BY c.ordinal_position").bind(&schema.name).bind(&name).fetch_all(pool).await?;
                        let columns = rows
                            .iter()
                            .map(|c| {
                                Ok(SchemaColumn {
                                    name: c.try_get("column_name")?,
                                    data_type: c.try_get("data_type")?,
                                    nullable: c.try_get::<String, _>("is_nullable")? == "YES",
                                    is_primary_key: c.try_get("pk")?,
                                })
                            })
                            .collect::<Result<Vec<_>>>()?;
                        schema.tables.push(TableInfo {
                            name,
                            is_view: t.try_get::<String, _>("table_type")? == "VIEW",
                            columns,
                        });
                    }
                    tree.schemas.push(schema);
                }
            }
            Pool::Mysql(pool) => {
                let schemas = sqlx::query("SELECT CAST(SCHEMA_NAME AS CHAR CHARACTER SET utf8mb4) AS SCHEMA_NAME FROM information_schema.SCHEMATA WHERE SCHEMA_NAME NOT IN ('information_schema','mysql','performance_schema','sys') ORDER BY SCHEMA_NAME").fetch_all(pool).await?;
                for s in schemas {
                    let name: String = s.try_get("SCHEMA_NAME")?;
                    let tables = sqlx::query("SELECT CAST(TABLE_NAME AS CHAR CHARACTER SET utf8mb4) AS TABLE_NAME, CAST(TABLE_TYPE AS CHAR CHARACTER SET utf8mb4) AS TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA=? ORDER BY TABLE_NAME").bind(&name).fetch_all(pool).await?;
                    let mut schema = SchemaInfo {
                        name,
                        tables: Vec::new(),
                    };
                    for t in tables {
                        let name: String = t.try_get("TABLE_NAME")?;
                        let rows = sqlx::query("SELECT CAST(COLUMN_NAME AS CHAR CHARACTER SET utf8mb4) AS COLUMN_NAME, CAST(COLUMN_TYPE AS CHAR CHARACTER SET utf8mb4) AS COLUMN_TYPE, CAST(IS_NULLABLE AS CHAR CHARACTER SET utf8mb4) AS IS_NULLABLE, CAST(COLUMN_KEY AS CHAR CHARACTER SET utf8mb4) AS COLUMN_KEY FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION").bind(&schema.name).bind(&name).fetch_all(pool).await?;
                        let columns = rows
                            .iter()
                            .map(|c| {
                                Ok(SchemaColumn {
                                    name: c.try_get("COLUMN_NAME")?,
                                    data_type: c.try_get("COLUMN_TYPE")?,
                                    nullable: c.try_get::<String, _>("IS_NULLABLE")? == "YES",
                                    is_primary_key: c.try_get::<String, _>("COLUMN_KEY")? == "PRI",
                                })
                            })
                            .collect::<Result<Vec<_>>>()?;
                        schema.tables.push(TableInfo {
                            name,
                            is_view: t.try_get::<String, _>("TABLE_TYPE")? == "VIEW",
                            columns,
                        });
                    }
                    tree.schemas.push(schema);
                }
            }
        }
        Ok(tree)
    }
}
