//! Conservative SQL result provenance and schema completion, without executing SQL.
use nozhik_database::{
    DatabaseSchemaTree, DbDriverType, DbValue, QueryResult, introspection::TableInfo,
};
use nozhik_language_support::lsp_types::{CompletionItem, CompletionItemKind};

#[derive(Clone, Debug)]
pub struct SelectSource {
    pub schema: String,
    pub table: String,
    pub defaults: Vec<(String, DbValue)>,
}

fn lex(sql: &str) -> Vec<String> {
    super::sql_console::tokens(sql)
        .into_iter()
        .filter_map(|(r, kind)| {
            let value = sql.get(r)?.trim();
            (!value.is_empty() && kind != Some("comment")).then(|| value.to_owned())
        })
        .collect()
}
const fn word(token: &str, keyword: &str) -> bool {
    token.eq_ignore_ascii_case(keyword)
}
fn identifier(token: &str) -> Option<String> {
    if let Some(quote) = token
        .chars()
        .next()
        .filter(|c| matches!(c, '"' | '`' | '['))
    {
        let end = if quote == '[' { ']' } else { quote };
        return token
            .strip_prefix(quote)?
            .strip_suffix(end)
            .map(|s| s.replace(&format!("{end}{end}"), &end.to_string()));
    }
    let mut chars = token.chars();
    (chars.next().is_some_and(|c| c.is_alphabetic() || c == '_')
        && chars.all(|c| c.is_alphanumeric() || c == '_' || c == '$'))
    .then(|| token.into())
}
fn relation(tokens: &[String], index: &mut usize) -> Option<(Option<String>, String)> {
    let first = identifier(tokens.get(*index)?)?;
    *index += 1;
    if tokens.get(*index).is_some_and(|s| s == ".") {
        *index += 1;
        let table = identifier(tokens.get(*index)?)?;
        *index += 1;
        Some((Some(first), table))
    } else {
        Some((None, first))
    }
}
fn resolve<'a>(
    tree: &'a DatabaseSchemaTree,
    schema: &str,
    explicit: Option<&str>,
    name: &str,
) -> Option<(&'a str, &'a TableInfo)> {
    let mut matches = tree
        .schemas
        .iter()
        .filter(|s| explicit.map_or_else(|| schema.is_empty() || s.name == schema, |n| s.name == n))
        .flat_map(|s| {
            s.tables
                .iter()
                .filter(move |t| t.name == name)
                .map(move |t| (s.name.as_str(), t))
        });
    let result = matches.next()?;
    matches.next().is_none().then_some(result)
}
/// Only direct, unaliased projections from one base relation can become writable.
#[allow(
    clippy::too_many_lines,
    reason = "Conservative provenance checks reject every unsupported SQL shape before enabling edits"
)]
pub fn select_source(
    sql: &str,
    schema: &str,
    tree: &DatabaseSchemaTree,
    result: &QueryResult,
) -> Option<SelectSource> {
    let mut t = lex(sql);
    if t.last().is_some_and(|s| s == ";") {
        t.pop();
    }
    if !word(t.first()?, "SELECT")
        || t.iter().skip(1).any(|s| {
            [
                "JOIN",
                "UNION",
                "INTERSECT",
                "EXCEPT",
                "GROUP",
                "HAVING",
                "DISTINCT",
                "INTO",
                "SELECT",
            ]
            .iter()
            .any(|k| word(s, k))
        })
        || t.iter().any(|s| s == ";")
    {
        return None;
    }
    let from = t.iter().position(|s| word(s, "FROM"))?;
    let mut i = from + 1;
    let (explicit, table) = relation(&t, &mut i)?;
    let (namespace, info) = resolve(tree, schema, explicit.as_deref(), &table)?;
    if info.is_view || !info.details_loaded {
        return None;
    }
    let mut alias = table.clone();
    if t.get(i).is_some_and(|s| word(s, "AS")) {
        i += 1;
        alias = identifier(t.get(i)?)?;
        i += 1;
    } else if t.get(i).is_some_and(|s| {
        !["WHERE", "ORDER", "LIMIT", "OFFSET", "FOR"]
            .iter()
            .any(|k| word(s, k))
    }) {
        alias = identifier(t.get(i)?)?;
        i += 1;
    }
    if t.get(i).is_some_and(|s| {
        !["WHERE", "ORDER", "LIMIT", "OFFSET", "FOR"]
            .iter()
            .any(|k| word(s, k))
    }) {
        return None;
    }
    let projection = &t[1..from];
    let mut columns = Vec::new();
    for part in projection.split(|s| s == ",") {
        let part = if part.len() == 3
            && part[1] == "."
            && identifier(&part[0]).is_some_and(|s| s == alias || s == table)
        {
            &part[2..]
        } else {
            part
        };
        if part.len() != 1 {
            return None;
        }
        if part[0] == "*" {
            columns.extend(info.columns.iter().map(|c| c.name.clone()));
        } else {
            columns.push(identifier(&part[0])?);
        }
    }
    if columns.len() != result.columns.len()
        || columns
            .iter()
            .zip(&result.columns)
            .any(|(a, b)| *a != b.name || !info.columns.iter().any(|c| c.name == *a))
        || columns
            .iter()
            .enumerate()
            .any(|(i, c)| columns[..i].contains(c))
    {
        return None;
    }
    // Require the complete primary key for projected snapshots. Full snapshots without
    // a key still use the existing exactly-one-row optimistic concurrency check.
    if info
        .columns
        .iter()
        .any(|c| c.is_primary_key && !columns.contains(&c.name))
        || (columns.len() != info.columns.len() && !info.columns.iter().any(|c| c.is_primary_key))
    {
        return None;
    }
    let defaults = t
        .iter()
        .position(|s| word(s, "WHERE"))
        .map_or_else(Vec::new, |start| {
            let end = t[start + 1..]
                .iter()
                .position(|s| {
                    ["ORDER", "LIMIT", "OFFSET", "FOR"]
                        .iter()
                        .any(|k| word(s, k))
                })
                .map_or(t.len(), |n| start + 1 + n);
            equalities(&t[start + 1..end], &alias, info)
        });
    Some(SelectSource {
        schema: namespace.into(),
        table,
        defaults,
    })
}
fn unparen(mut t: &[String]) -> &[String] {
    loop {
        if t.first().is_none_or(|s| s != "(") {
            return t;
        }
        let mut depth = 0;
        let closes_at_end = t.iter().enumerate().all(|(i, s)| {
            if s == "(" {
                depth += 1;
            } else if s == ")" {
                depth -= 1;
            }
            depth != 0 || i == t.len() - 1
        });
        if !closes_at_end || depth != 0 {
            return t;
        }
        t = &t[1..t.len() - 1];
    }
}
fn literal(t: &[String]) -> Option<DbValue> {
    if t.len() == 2 && (t[0] == "-" || t[0] == "+") {
        return literal(&[format!("{}{}", t[0], t[1])]);
    }
    let s = t.first().filter(|_| t.len() == 1)?;
    if let Some(s) = s.strip_prefix('\'').and_then(|s| s.strip_suffix('\'')) {
        // Backslash semantics vary by server mode; do not guess those literals.
        return (!s.contains('\\')).then(|| DbValue::Text(s.replace("''", "'")));
    }
    if word(s, "TRUE") || word(s, "FALSE") {
        return Some(DbValue::Bool(word(s, "TRUE")));
    }
    if let Ok(v) = s.parse::<i64>() {
        return Some(DbValue::Int(v));
    }
    s.parse::<f64>()
        .ok()
        .filter(|v| v.is_finite())
        .map(DbValue::Float)
}
fn equalities(t: &[String], alias: &str, info: &TableInfo) -> Vec<(String, DbValue)> {
    let mut values = gather_equalities(t, alias, info);
    let conflicts: Vec<_> = values
        .iter()
        .filter(|(n, v)| values.iter().any(|(m, w)| n == m && v != w))
        .map(|(n, _)| n.clone())
        .collect();
    let mut seen = std::collections::BTreeSet::new();
    values.retain(|(name, _)| !conflicts.contains(name) && seen.insert(name.clone()));
    values
}
fn gather_equalities(t: &[String], alias: &str, info: &TableInfo) -> Vec<(String, DbValue)> {
    let t = unparen(t);
    // An OR/NOT anywhere invalidates defaults, even inside nested expressions.
    if t.iter().any(|s| {
        ["OR", "NOT", "SELECT", "BETWEEN", "CASE"]
            .iter()
            .any(|k| word(s, k))
    }) {
        return Vec::new();
    }
    let mut depth = 0;
    let splits: Vec<_> = t
        .iter()
        .enumerate()
        .filter_map(|(i, s)| {
            if s == "(" {
                depth += 1;
            } else if s == ")" {
                depth -= 1;
            }
            (depth == 0 && word(s, "AND")).then_some(i)
        })
        .collect();
    if !splits.is_empty() {
        let mut out = Vec::new();
        let mut start = 0;
        for end in splits.into_iter().chain(std::iter::once(t.len())) {
            out.extend(gather_equalities(&t[start..end], alias, info));
            start = end + 1;
        }
        return out;
    }
    let Some(eq) = t.iter().position(|s| s == "=") else {
        return Vec::new();
    };
    let column = |tokens: &[String]| {
        let tokens = if tokens.len() == 3
            && tokens[1] == "."
            && identifier(&tokens[0]).as_deref() == Some(alias)
        {
            &tokens[2..]
        } else {
            tokens
        };
        (tokens.len() == 1)
            .then(|| identifier(&tokens[0]))
            .flatten()
            .filter(|n| info.columns.iter().any(|c| c.name == *n))
    };
    column(&t[..eq])
        .zip(literal(&t[eq + 1..]))
        .or_else(|| column(&t[eq + 1..]).zip(literal(&t[..eq])))
        .into_iter()
        .collect()
}

pub fn completions(
    source: &str,
    caret: usize,
    tree: &DatabaseSchemaTree,
    schema: &str,
    driver: DbDriverType,
) -> Vec<CompletionItem> {
    let before = source.get(..caret).unwrap_or("");
    if super::sql_console::tokens(source)
        .iter()
        .any(|(r, k)| r.start < caret && caret <= r.end && matches!(k, Some("comment" | "string")))
    {
        return Vec::new();
    }
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
        .map_or(0, |(i, c)| i + c.len_utf8());
    let prefix = before[start..].to_lowercase();
    let qualifier = before[..start]
        .strip_suffix('.')
        .and_then(|s| s.rsplit(|c: char| !c.is_alphanumeric() && c != '_').next());
    let tokens = lex(super::sql_console::statement_at(source, caret));
    let mut relations = Vec::new();
    for (i, _) in tokens.iter().enumerate().filter(|(_, s)| {
        ["FROM", "JOIN", "UPDATE", "INTO"]
            .iter()
            .any(|k| word(s, k))
    }) {
        let mut next = i + 1;
        if let Some((explicit, name)) = relation(&tokens, &mut next)
            && let Some((namespace, info)) = resolve(tree, schema, explicit.as_deref(), &name)
        {
            if tokens.get(next).is_some_and(|s| word(s, "AS")) {
                next += 1;
            }
            let alias = tokens
                .get(next)
                .and_then(|s| identifier(s))
                .unwrap_or_else(|| name.clone());
            relations.push((namespace, info, alias));
        }
    }
    let mut items = Vec::new();
    let table_context = lex(&before[..start]).last().is_some_and(|s| {
        ["FROM", "JOIN", "UPDATE", "INTO"]
            .iter()
            .any(|k| word(s, k))
    });
    for namespace in &tree.schemas {
        if qualifier.is_none() && !schema.is_empty() && namespace.name != schema {
            continue;
        }
        for table in &namespace.tables {
            if (qualifier.is_none() || qualifier == Some(namespace.name.as_str()))
                && table.name.to_lowercase().starts_with(&prefix)
            {
                items.push(item(
                    &table.name,
                    &format!("{}.{}", namespace.name, table.name),
                    CompletionItemKind::CLASS,
                    driver,
                ));
            }
            if !table_context
                && relations.iter().any(|(s, t, a)| {
                    *s == namespace.name
                        && t.name == table.name
                        && qualifier.is_none_or(|q| q == a || q == table.name)
                })
            {
                for c in &table.columns {
                    if c.name.to_lowercase().starts_with(&prefix) {
                        items.push(item(
                            &c.name,
                            &format!("{}.{} · {}", table.name, c.name, c.data_type),
                            CompletionItemKind::FIELD,
                            driver,
                        ));
                    }
                }
            }
        }
    }
    items.sort_by(|a, b| a.label.cmp(&b.label));
    items.dedup_by(|a, b| a.label == b.label && a.detail == b.detail);
    items.truncate(200);
    items
}
fn item(
    name: &str,
    detail: &str,
    kind: CompletionItemKind,
    driver: DbDriverType,
) -> CompletionItem {
    CompletionItem {
        label: name.into(),
        detail: Some(detail.into()),
        kind: Some(kind),
        insert_text: Some(nozhik_database::connection::quote_identifier(driver, name)),
        ..CompletionItem::default()
    }
}
/// Relations whose column metadata should be fetched lazily for this console.
pub fn referenced_tables(
    sql: &str,
    schema: &str,
    tree: &DatabaseSchemaTree,
) -> Vec<(String, TableInfo)> {
    let t = lex(sql);
    let mut out = Vec::new();
    for (i, _) in t.iter().enumerate().filter(|(_, s)| {
        ["FROM", "JOIN", "UPDATE", "INTO"]
            .iter()
            .any(|k| word(s, k))
    }) {
        if let Some((explicit, name)) = relation(&t, &mut (i + 1))
            && let Some((s, info)) = resolve(tree, schema, explicit.as_deref(), &name)
            && !info.details_loaded
        {
            out.push((s.into(), info.clone()));
        }
    }
    out
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use nozhik_database::{
        ColumnInfo,
        introspection::{SchemaColumn, SchemaInfo},
    };
    pub fn fixture() -> (DatabaseSchemaTree, QueryResult) {
        let names = ["id", "type", "name"];
        let info = TableInfo {
            name: "goods".into(),
            is_view: false,
            details_loaded: true,
            objects: nozhik_database::catalog::TableObjects::default(),
            columns: names
                .iter()
                .map(|n| SchemaColumn {
                    name: (*n).into(),
                    data_type: "INTEGER".into(),
                    nullable: true,
                    is_primary_key: *n == "id",
                    is_foreign_key: false,
                    default_value: None,
                })
                .collect(),
        };
        (
            DatabaseSchemaTree {
                schemas: vec![SchemaInfo {
                    name: "main".into(),
                    tables: vec![info],
                }],
            },
            QueryResult {
                columns: names
                    .iter()
                    .map(|n| ColumnInfo {
                        name: (*n).into(),
                        data_type: "INTEGER".into(),
                    })
                    .collect(),
                ..Default::default()
            },
        )
    }
    #[test]
    fn direct_select_provenance_and_exact_defaults() -> Result<(), Box<dyn std::error::Error>> {
        let (tree, result) = fixture();
        for predicate in [
            "type=11",
            "(type = 11)",
            "11 = type",
            "((g.type=11) AND name='it''s')",
            "type=11 AND id>2",
        ] {
            let source = select_source(
                &format!(
                    "SELECT g.* FROM main.goods AS g WHERE {predicate} ORDER BY id DESC LIMIT 30;"
                ),
                "",
                &tree,
                &result,
            )
            .ok_or_else(|| predicate.to_owned())?;
            assert!(
                source.defaults.contains(&("type".into(), DbValue::Int(11))),
                "{predicate}: {:?}",
                source.defaults
            );
        }
        for predicate in [
            "type>11",
            "type<11",
            "type!=11",
            "type>=11",
            "type<=11",
            "type=11 OR id=1",
            "NOT(type=11)",
            "type IN (11)",
            "type=11+1",
            "type=11 AND type=12",
            "type=11 AND (type=12 AND type=13)",
            "type=other",
            "type IS NULL",
        ] {
            let source = select_source(
                &format!("SELECT * FROM goods WHERE {predicate}"),
                "main",
                &tree,
                &result,
            )
            .ok_or_else(|| predicate.to_owned())?;
            assert!(
                source.defaults.is_empty(),
                "{predicate}: {:?}",
                source.defaults
            );
        }
        let source = select_source(
            "SELECT * FROM goods WHERE type=11 AND name='x' AND type=11",
            "main",
            &tree,
            &result,
        )
        .ok_or("duplicate constraints")?;
        assert_eq!(
            source.defaults.iter().filter(|(n, _)| n == "type").count(),
            1
        );
        for sql in [
            "SELECT DISTINCT * FROM goods",
            "SELECT a.* FROM goods a JOIN goods b ON a.id=b.id",
            "SELECT * FROM goods UNION SELECT * FROM goods",
            "SELECT * FROM goods; SELECT * FROM goods",
            "SELECT count(*) FROM goods",
            "WITH x AS (SELECT * FROM goods) SELECT * FROM x",
            "SELECT * FROM goods, goods",
            "SELECT * FROM goods WHERE id IN (SELECT id FROM goods)",
            "SELECT type AS id,type,name FROM goods",
            "SELECT * FROM goods GROUP BY id",
        ] {
            assert!(
                select_source(sql, "main", &tree, &result).is_none(),
                "{sql}"
            );
        }
        Ok(())
    }
    #[test]
    fn schema_completion_is_scoped_and_skips_comments_and_strings() {
        let (mut tree, _) = fixture();
        let mut other = tree.schemas[0].tables[0].clone();
        other.name = "unrelated".into();
        other.columns[0].name = "foreign_field".into();
        tree.schemas[0].tables.push(other);
        for (sql, caret, expected) in [
            ("SELECT * FROM go", 17, "goods"),
            ("SELECT * FROM goods g WHERE g.ty", 30, "type"),
            ("SELECT na FROM goods", 9, "name"),
        ] {
            let caret = caret.min(sql.len());
            assert!(
                completions(sql, caret, &tree, "main", DbDriverType::Sqlite)
                    .iter()
                    .any(|c| c.label == expected),
                "{sql} caret {caret}"
            );
        }
        for sql in [
            "SELECT * FROM goods WHERE name='ty",
            "SELECT * FROM goods -- ty",
            "SELECT * FROM goods /* ty",
        ] {
            assert_eq!(
                completions(sql, sql.len(), &tree, "main", DbDriverType::Sqlite),
                []
            );
        }
        let sql = "SELECT * FROM goods WHERE fo";
        assert!(
            !completions(sql, sql.len(), &tree, "main", DbDriverType::Sqlite)
                .iter()
                .any(|c| c.label == "foreign_field")
        );
    }
    #[tokio::test]
    async fn projected_result_edits_use_query_defaults_and_transactional_changes()
    -> Result<(), Box<dyn std::error::Error>> {
        use nozhik_database::{CancellationToken, Connection, ConnectionProfile};
        let db = Connection::connect(
            &ConnectionProfile {
                name: "console".into(),
                ..Default::default()
            },
            None,
            None,
        )
        .await?;
        let cancel = CancellationToken::new();
        db.query("CREATE TABLE goods(id INTEGER PRIMARY KEY,type INTEGER,name TEXT); INSERT INTO goods VALUES(1,11,'first'),(2,12,'other');",None,&cancel).await?;
        let tree = db.introspect().await?;
        let sql = "SELECT id,name FROM goods WHERE type=11 ORDER BY id";
        let results = db.query(sql, Some("main"), &cancel).await?;
        let mut console = super::super::sql_console::SqlConsole::new(
            500,
            "test".into(),
            "main".into(),
            sql,
            "test".into(),
        );
        console.pending_sql = sql.into();
        console.loaded_results(results, Some(&tree), DbDriverType::Sqlite);
        let changes = console
            .active_table_mut()
            .and_then(|t| t.changes.as_mut())
            .ok_or("changes")?;
        changes.rows[0].values[1] = Some(DbValue::Text("updated".into()));
        changes.add_row();
        changes.rows[1].values[1] = Some(DbValue::Text("new".into()));
        assert!(
            changes
                .statements()?
                .iter()
                .any(|s| s.contains("\"type\"") && s.contains("11"))
        );
        db.apply_changes(changes, &cancel).await?;
        let results = db.query(sql, Some("main"), &cancel).await?;
        console.loaded_results(results, Some(&tree), DbDriverType::Sqlite);
        let changes = console
            .active_table_mut()
            .and_then(|t| t.changes.as_mut())
            .ok_or("changes")?;
        assert_eq!(changes.rows.len(), 2);
        assert_eq!(
            changes.rows[0].values[1],
            Some(DbValue::Text("updated".into()))
        );
        changes.rows[0].deleted = true;
        db.apply_changes(changes, &cancel).await?;
        let result = db
            .query("SELECT type,name FROM goods ORDER BY id", None, &cancel)
            .await?;
        assert_eq!(
            result[0].rows,
            vec![
                vec![DbValue::Int(12), DbValue::Text("other".into())],
                vec![DbValue::Int(11), DbValue::Text("new".into())]
            ]
        );
        db.close().await;
        Ok(())
    }
}
