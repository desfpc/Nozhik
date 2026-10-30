//! Catalog metadata using bound names and dialect-specific system catalogs.
use crate::{
    Connection, Result,
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
    /// Keys, references, indexes and catalog DDL.
    pub objects: crate::catalog::TableObjects,
    /// True after relation details were fetched; empty catalogs are otherwise placeholders.
    pub details_loaded: bool,
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
    /// Read namespace and relation names without querying columns, indexes or DDL.
    /// # Errors
    /// Returns catalog permission, transport or decoding errors.
    pub async fn catalog(&self) -> Result<DatabaseSchemaTree> {
        let mut tree = DatabaseSchemaTree::default();
        let schemas:Vec<String>=match &self.pool {
            Pool::Sqlite(p)=>sqlx::query("PRAGMA database_list").fetch_all(p).await?.iter().map(|r|r.try_get("name")).collect::<std::result::Result<_,_>>()?,
            Pool::Postgres(p)=>sqlx::query_scalar("SELECT schema_name FROM information_schema.schemata WHERE schema_name NOT IN ('pg_catalog','information_schema') AND schema_name NOT LIKE 'pg_toast%' ORDER BY schema_name").fetch_all(p).await?,
            Pool::Mysql(p)=>sqlx::query_scalar("SELECT CAST(SCHEMA_NAME AS CHAR CHARACTER SET utf8mb4) FROM information_schema.SCHEMATA WHERE SCHEMA_NAME NOT IN ('information_schema','mysql','performance_schema','sys') ORDER BY SCHEMA_NAME").fetch_all(p).await?,
        };
        for schema in schemas {
            let relations:Vec<(String,String)>=match &self.pool {
                Pool::Sqlite(p)=>sqlx::query_as(&format!("SELECT name,type FROM {}.sqlite_master WHERE type IN ('table','view') AND name NOT LIKE 'sqlite_%' ORDER BY name",quote_identifier(self.driver,&schema))).fetch_all(p).await?,
                Pool::Postgres(p)=>sqlx::query_as("SELECT table_name,table_type FROM information_schema.tables WHERE table_schema=$1 ORDER BY table_name").bind(&schema).fetch_all(p).await?,
                Pool::Mysql(p)=>sqlx::query_as("SELECT CAST(TABLE_NAME AS CHAR CHARACTER SET utf8mb4),CAST(TABLE_TYPE AS CHAR CHARACTER SET utf8mb4) FROM information_schema.TABLES WHERE TABLE_SCHEMA=? ORDER BY TABLE_NAME").bind(&schema).fetch_all(p).await?,
            };
            tree.schemas.push(SchemaInfo {
                name: schema,
                tables: relations
                    .into_iter()
                    .map(|(name, kind)| TableInfo {
                        name,
                        is_view: kind.eq_ignore_ascii_case("view"),
                        columns: Vec::new(),
                        objects: crate::catalog::TableObjects::default(),
                        details_loaded: false,
                    })
                    .collect(),
            });
        }
        Ok(tree)
    }
    /// Read one relation's columns, keys, indexes and DDL. Names remain quoted/bound.
    /// # Errors
    /// Returns catalog permission, missing relation or transport errors.
    pub async fn describe_table(
        &self,
        schema: &str,
        table: &str,
        is_view: bool,
    ) -> Result<TableInfo> {
        let columns = match &self.pool {
            Pool::Sqlite(p) => {
                let rows = sqlx::query(&format!(
                    "PRAGMA {}.table_info({})",
                    quote_identifier(self.driver, schema),
                    quote_identifier(self.driver, table)
                ))
                .fetch_all(p)
                .await?;
                let single_pk = rows
                    .iter()
                    .filter(|r| r.try_get::<i64, _>("pk").is_ok_and(|v| v > 0))
                    .count()
                    == 1;
                rows.iter()
                    .map(|r| {
                        let pk = r.try_get::<i64, _>("pk")? > 0;
                        let data_type: String = r.try_get("type")?;
                        let rowid_pk = pk && single_pk && data_type.eq_ignore_ascii_case("INTEGER");
                        Ok(SchemaColumn {
                            name: r.try_get("name")?,
                            data_type,
                            nullable: r.try_get::<i64, _>("notnull")? == 0 && !rowid_pk,
                            is_primary_key: pk,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            Pool::Postgres(p) => {
                let rows=sqlx::query("SELECT c.column_name, c.data_type, c.is_nullable, EXISTS (SELECT 1 FROM information_schema.table_constraints tc JOIN information_schema.key_column_usage k ON tc.constraint_catalog=k.constraint_catalog AND tc.constraint_schema=k.constraint_schema AND tc.constraint_name=k.constraint_name AND tc.table_name=k.table_name WHERE tc.constraint_type='PRIMARY KEY' AND k.table_schema=c.table_schema AND k.table_name=c.table_name AND k.column_name=c.column_name) AS pk FROM information_schema.columns c WHERE c.table_schema=$1 AND c.table_name=$2 ORDER BY c.ordinal_position").bind(schema).bind(table).fetch_all(p).await?;
                rows.iter()
                    .map(|r| {
                        Ok(SchemaColumn {
                            name: r.try_get("column_name")?,
                            data_type: r.try_get("data_type")?,
                            nullable: r.try_get::<String, _>("is_nullable")? == "YES",
                            is_primary_key: r.try_get("pk")?,
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
            Pool::Mysql(p) => {
                let rows=sqlx::query("SELECT CAST(COLUMN_NAME AS CHAR CHARACTER SET utf8mb4) AS name, CAST(CONCAT(COLUMN_TYPE, IF(EXTRA='','',CONCAT(' ',EXTRA))) AS CHAR CHARACTER SET utf8mb4) AS kind, CAST(IS_NULLABLE AS CHAR CHARACTER SET utf8mb4) AS nullable, CAST(COLUMN_KEY AS CHAR CHARACTER SET utf8mb4) AS pk FROM information_schema.COLUMNS WHERE TABLE_SCHEMA=? AND TABLE_NAME=? ORDER BY ORDINAL_POSITION").bind(schema).bind(table).fetch_all(p).await?;
                rows.iter()
                    .map(|r| {
                        Ok(SchemaColumn {
                            name: r.try_get("name")?,
                            data_type: r.try_get("kind")?,
                            nullable: r.try_get::<String, _>("nullable")? == "YES",
                            is_primary_key: r.try_get::<String, _>("pk")? == "PRI",
                        })
                    })
                    .collect::<Result<Vec<_>>>()?
            }
        };
        let objects = self
            .table_objects(schema, table, is_view)
            .await
            .unwrap_or_else(|e| crate::catalog::TableObjects {
                error: Some(e.to_string()),
                ..Default::default()
            });
        Ok(TableInfo {
            name: table.into(),
            is_view,
            columns,
            objects,
            details_loaded: true,
        })
    }
    /// Read a complete metadata snapshot. Interactive clients should use `catalog` and `describe_table`.
    /// # Errors
    /// Returns catalog, permission or transport errors for any relation.
    pub async fn introspect(&self) -> Result<DatabaseSchemaTree> {
        let mut tree = self.catalog().await?;
        for schema in &mut tree.schemas {
            for table in &mut schema.tables {
                *table = self
                    .describe_table(&schema.name, &table.name, table.is_view)
                    .await?;
            }
        }
        Ok(tree)
    }
}
