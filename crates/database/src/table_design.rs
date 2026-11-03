//! Editable table snapshots and dialect-specific DDL migrations.
use crate::{DbDriverType, Error, Result, connection::quote_identifier};
mod catalog;
mod plan;
mod sql;
pub use plan::migration_sql;

/// A column, key, index or trigger with stable identity within a draft.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Element {
    /// Draft identity; new elements must have fresh IDs.
    pub id: u64,
    /// Native identifier.
    pub name: String,
    /// Object comment when supported by the engine.
    pub comment: String,
    /// Editable kind-specific properties.
    pub data: Properties,
    /// Original native clause, retained verbatim while the object is unchanged.
    pub native_sql: String,
}
/// Kind-specific table object properties.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Properties {
    /// Column definition.
    Column(Column),
    /// Primary/unique/foreign/check/exclusion constraint.
    Key(Key),
    /// Independent index (constraint-owned indexes are edited through their keys).
    Index(Index),
    /// Trigger definition.
    Trigger(Trigger),
}
/// Column properties. SQL expressions remain explicit, unexecuted draft text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent SQL column attributes"
)]
pub struct Column {
    /// Native type, including length/precision/unsigned qualifiers.
    pub data_type: String,
    /// NOT NULL constraint.
    pub not_null: bool,
    /// Default SQL expression; empty means no default.
    pub default_sql: String,
    /// Native collation identifier, possibly qualified.
    pub collation: String,
    /// Empty, ALWAYS or BY DEFAULT for PostgreSQL identity.
    pub identity: String,
    /// Identity sequence options (START/INCREMENT/MINVALUE/MAXVALUE/CACHE/CYCLE).
    pub identity_options: String,
    /// Existing identity sequence, qualified by the native catalog.
    pub identity_sequence: String,
    /// MySQL `AUTO_INCREMENT` / SQLite AUTOINCREMENT.
    pub auto_increment: bool,
    /// SQLite inline primary key.
    pub primary: bool,
    /// SQLite inline unique constraint.
    pub unique: bool,
    /// Generated expression; empty means ordinary stored value.
    pub generated_sql: String,
    /// Stored rather than virtual generated expression.
    pub stored: bool,
    /// Additional native column clauses, retained in the draft.
    pub extra_sql: String,
    /// PostgreSQL type conversion expression for ALTER TYPE.
    pub using_sql: String,
}
/// One ordered index/key column or expression.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexPart {
    /// Column name or SQL expression.
    pub expression: String,
    /// Empty / ASC / DESC, optionally NULLS FIRST/LAST.
    pub order: String,
    /// Native collation identifier.
    pub collation: String,
    /// PostgreSQL operator class.
    pub operator_class: String,
    /// PostgreSQL exclusion operator.
    pub exclude_operator: String,
}
/// Constraint category.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeyKind {
    /// Primary key.
    #[default]
    Primary,
    /// Unique constraint.
    Unique,
    /// Foreign key.
    Foreign,
    /// Check constraint.
    Check,
    /// PostgreSQL exclusion constraint.
    Exclusion,
}
/// Table constraint properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Key {
    /// Constraint category.
    pub kind: KeyKind,
    /// Local ordered columns / exclusion expressions.
    pub columns: Vec<IndexPart>,
    /// Referenced namespace; empty uses the current schema.
    pub referenced_schema: String,
    /// Referenced relation.
    pub referenced_table: String,
    /// Referenced column names.
    pub referenced_columns: Vec<String>,
    /// Empty / NO ACTION / RESTRICT / CASCADE / SET NULL / SET DEFAULT.
    pub on_delete: String,
    /// Foreign-key MATCH type (empty uses the engine default).
    pub match_type: String,
    /// Update referential action.
    pub on_update: String,
    /// CHECK expression.
    pub expression: String,
    /// PostgreSQL exclusion access method.
    pub method: String,
    /// Deferrable constraint.
    pub deferrable: bool,
    /// Initially deferred constraint.
    pub deferred: bool,
    /// Additional native constraint clauses.
    pub extra_sql: String,
}
/// Independent index properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Index {
    /// Unique index.
    pub unique: bool,
    /// Ordered expressions/columns.
    pub columns: Vec<IndexPart>,
    /// Access method (btree/hash/gin/gist, or FULLTEXT/SPATIAL for MySQL).
    pub method: String,
    /// PostgreSQL INCLUDE columns.
    pub include_columns: Vec<String>,
    /// Partial-index predicate.
    pub condition: String,
    /// Additional native index options.
    pub extra_sql: String,
}
/// Trigger properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent trigger firing, constraint and creation-mode attributes"
)]
pub struct Trigger {
    /// BEFORE / AFTER / INSTEAD OF.
    pub timing: String,
    /// INSERT / UPDATE / DELETE / TRUNCATE, joined by OR where supported.
    pub events: String,
    /// PostgreSQL/SQLite UPDATE OF columns.
    pub update_columns: Vec<String>,
    /// FOR EACH ROW rather than STATEMENT.
    pub for_each_row: bool,
    /// Trigger WHEN condition.
    pub condition: String,
    /// Native body (BEGIN…END or PostgreSQL EXECUTE FUNCTION…).
    pub body_sql: String,
    /// MySQL account clause; empty uses the current login.
    pub definer_sql: String,
    /// MySQL creation SQL mode.
    pub sql_mode: String,
    /// Preserve even an explicitly empty MySQL creation mode.
    pub preserve_sql_mode: bool,
    /// PostgreSQL constraint trigger.
    pub constraint: bool,
    /// Deferrable constraint trigger.
    pub deferrable: bool,
    /// Initially deferred constraint trigger.
    pub deferred: bool,
    /// PostgreSQL constraint-trigger FROM relation.
    pub referenced_table: String,
    /// PostgreSQL ENABLE / DISABLE / ENABLE REPLICA / ENABLE ALWAYS.
    pub enabled: String,
    /// Additional trigger clauses.
    pub extra_sql: String,
}
/// Persistent table options plus its editable objects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[allow(
    clippy::struct_excessive_bools,
    reason = "Independent table storage and row-security attributes"
)]
pub struct Definition {
    /// Native dialect.
    pub driver: DbDriverType,
    /// Namespace (not changed by a table edit).
    pub schema: String,
    /// Relation identifier.
    pub name: String,
    /// Table comment.
    pub comment: String,
    /// MySQL storage engine.
    pub engine: String,
    /// MySQL default table collation.
    pub collation: String,
    /// MySQL row format.
    pub row_format: String,
    /// MySQL next `AUTO_INCREMENT` value; empty leaves the counter untouched.
    pub auto_increment: String,
    /// PostgreSQL owner.
    pub owner: String,
    /// PostgreSQL tablespace; empty means server default.
    pub tablespace: String,
    /// PostgreSQL table access method.
    pub access_method: String,
    /// PostgreSQL storage parameters.
    pub storage_options: String,
    /// PostgreSQL unlogged persistence.
    pub unlogged: bool,
    /// PostgreSQL row-level security.
    pub row_security: bool,
    /// PostgreSQL forced row-level security.
    pub force_row_security: bool,
    /// SQLite STRICT table.
    pub strict: bool,
    /// SQLite WITHOUT ROWID table.
    pub without_rowid: bool,
    /// Native partition/inheritance options retained by ALTER migrations.
    pub partition_sql: String,
    /// Other native table options.
    pub extra_sql: String,
    /// Editable objects, preserving column order.
    pub elements: Vec<Element>,
    /// Explicit table privileges visible to this login.
    pub grants: Vec<TableGrant>,
    /// Original CREATE TABLE, used to detect SQLite catalog races.
    pub source_sql: String,
    /// SQLite `schema_version` at load time.
    pub catalog_revision: i64,
    /// SQLite connection foreign-key enforcement to restore after migration.
    pub sqlite_foreign_keys: bool,
    /// SQLite legacy ALTER mode to restore after migration.
    pub sqlite_legacy_alter: bool,
    /// Server version for capability-specific syntax.
    pub server_version: String,
}
/// Explicit table-level access privilege.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableGrant {
    /// PostgreSQL role / MySQL account user (PUBLIC is PostgreSQL's public role).
    pub user: String,
    /// MySQL account host.
    pub host: String,
    /// One engine table privilege.
    pub privilege: String,
    /// WITH GRANT OPTION.
    pub grant_option: bool,
}
impl Definition {
    /// Allocate a draft identity without reusing a loaded element's ID.
    #[must_use]
    pub fn next_id(&self) -> u64 {
        self.elements.iter().map(|e| e.id).max().unwrap_or(0) + 1
    }
    /// True if only automatic data counters changed since this snapshot.
    #[must_use]
    pub fn same_schema(&self, other: &Self) -> bool {
        let mut a = self.clone();
        let mut b = other.clone();
        a.auto_increment.clear();
        b.auto_increment.clear();
        a.catalog_revision = 0;
        b.catalog_revision = 0;
        a.source_sql.clear();
        b.source_sql.clear();
        a == b
    }
    /// Qualified native relation name.
    #[must_use]
    pub fn qualified_name(&self) -> String {
        format!(
            "{}.{}",
            quote_identifier(self.driver, &self.schema),
            quote_identifier(self.driver, &self.name)
        )
    }
}
impl Element {
    /// Create an unnamed draft column with a fresh identity.
    #[must_use]
    pub fn column(id: u64, name: String) -> Self {
        Self {
            id,
            name,
            comment: String::new(),
            native_sql: String::new(),
            data: Properties::Column(Column {
                data_type: "INTEGER".into(),
                ..Column::default()
            }),
        }
    }
}
fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

#[cfg(test)]
mod tests;
