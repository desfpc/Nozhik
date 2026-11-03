//! Small SQL lexer for catalog clauses; strings/comments never become identifiers.
use super::{
    Column, Element, Index, IndexPart, Key, KeyKind, Properties, Result, Trigger, invalid,
};
use std::ops::Range;
#[derive(Clone, Debug)]
pub(super) struct Token {
    pub range: Range<usize>,
    pub value: String,
    pub depth: usize,
    pub quoted: bool,
    pub literal: bool,
}
impl Token {
    pub fn is(&self, s: &str) -> bool {
        !self.quoted && !self.literal && self.value.eq_ignore_ascii_case(s)
    }
}
pub(super) fn tokens(sql: &str) -> Result<Vec<Token>> {
    lex(sql, false)
}
#[allow(
    clippy::too_many_lines,
    reason = "One lexical state machine preserves SQL quotes, comments and parentheses"
)]
pub(super) fn lex(sql: &str, backslash: bool) -> Result<Vec<Token>> {
    let b = sql.as_bytes();
    let mut i = 0;
    let mut depth = 0usize;
    let mut out = Vec::new();
    while i < b.len() {
        let start = i;
        let byte = b[i];
        if byte.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if b.get(i..i + 2) == Some(b"--") || byte == b'#' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if b.get(i..i + 2) == Some(b"/*") {
            i += 2;
            let mut nesting = 1;
            while i < b.len() && nesting > 0 {
                if b.get(i..i + 2) == Some(b"/*") {
                    nesting += 1;
                    i += 2;
                } else if b.get(i..i + 2) == Some(b"*/") {
                    nesting -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            if nesting > 0 {
                return Err(invalid("Unclosed SQL comment"));
            }
            continue;
        }
        if b"'\"`[".contains(&byte) {
            let end = if byte == b'[' { b']' } else { byte };
            i += 1;
            let mut value = String::new();
            let mut closed = false;
            while i < b.len() {
                if b[i] == end {
                    if b.get(i + 1) == Some(&end) {
                        value.push(char::from(end));
                        i += 2;
                    } else {
                        i += 1;
                        closed = true;
                        break;
                    }
                } else if (backslash || (start > 0 && matches!(b[start - 1], b'E' | b'e')))
                    && b[i] == b'\\'
                    && byte == b'\''
                    && i + 1 < b.len()
                {
                    value.push(char::from(b[i + 1]));
                    i += 2;
                } else {
                    let c = sql[i..]
                        .chars()
                        .next()
                        .ok_or_else(|| invalid("Invalid SQL text"))?;
                    value.push(c);
                    i += c.len_utf8();
                }
            }
            if !closed {
                return Err(invalid("Unclosed SQL quote"));
            }
            out.push(Token {
                range: start..i,
                value,
                depth,
                quoted: byte != b'\'',
                literal: byte == b'\'',
            });
            continue;
        }
        if byte == b'$' {
            let mut j = i + 1;
            while b
                .get(j)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_')
            {
                j += 1;
            }
            if b.get(j) == Some(&b'$') {
                let tag = &sql[i..=j];
                let end = sql[j + 1..]
                    .find(tag)
                    .ok_or_else(|| invalid("Unclosed SQL dollar string"))?
                    + j
                    + 1
                    + tag.len();
                i = end;
                out.push(Token {
                    range: start..i,
                    value: sql[start..i].into(),
                    depth,
                    quoted: false,
                    literal: true,
                });
                continue;
            }
        }
        if byte == b')' {
            depth = depth
                .checked_sub(1)
                .ok_or_else(|| invalid("Unbalanced SQL parentheses"))?;
        }
        i += 1;
        if byte.is_ascii_alphanumeric() || byte == b'_' || byte >= 128 {
            while b
                .get(i)
                .is_some_and(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'$' || *c >= 128)
            {
                i += 1;
            }
        }
        out.push(Token {
            range: start..i,
            value: sql[start..i].into(),
            depth,
            quoted: false,
            literal: false,
        });
        if byte == b'(' {
            depth += 1;
        }
    }
    if depth != 0 {
        return Err(invalid("Unbalanced SQL parentheses"));
    }
    Ok(out)
}
pub(super) fn fragment(sql: &str) -> Result<()> {
    if tokens(sql)?.iter().any(|t| t.is(";")) {
        return Err(invalid(
            "Use one SQL expression/definition, without statement separators",
        ));
    }
    Ok(())
}
pub(super) fn parts(sql: &str) -> Result<Vec<String>> {
    let ts = tokens(sql)?;
    let mut start = 0;
    let mut out = Vec::new();
    for t in ts {
        if t.depth == 0 && t.is(",") {
            out.push(sql[start..t.range.start].trim().into());
            start = t.range.end;
        }
    }
    if !sql[start..].trim().is_empty() {
        out.push(sql[start..].trim().into());
    }
    Ok(out)
}
pub(super) fn parentheses(sql: &str) -> Result<(String, String)> {
    let ts = tokens(sql)?;
    let first = ts
        .iter()
        .find(|t| t.is("("))
        .ok_or_else(|| invalid(format!("Expected SQL column list in {sql}")))?;
    let last = ts
        .iter()
        .find(|t| t.is(")") && t.depth == first.depth && t.range.start > first.range.start)
        .ok_or_else(|| invalid("Unclosed SQL column list"))?;
    Ok((
        sql[first.range.end..last.range.start].into(),
        sql[last.range.end..].trim().trim_end_matches(';').into(),
    ))
}
fn value_after(ts: &[Token], word: &str) -> String {
    ts.iter()
        .position(|t| t.depth == 0 && t.is(word))
        .and_then(|i| ts.get(i + 1))
        .map_or_else(String::new, |t| t.value.clone())
}
pub(super) fn index_parts(sql: &str) -> Result<Vec<IndexPart>> {
    parts(sql)?
        .into_iter()
        .map(|part| {
            let ts = tokens(&part)?;
            let top = ts.iter().filter(|t| t.depth == 0).collect::<Vec<_>>();
            let mut end = part.len();
            let mut output = IndexPart::default();
            let mut i = 0;
            while i < top.len() {
                let t = top[i];
                if t.is("ASC") || t.is("DESC") || t.is("NULLS") {
                    end = end.min(t.range.start);
                    output.order = part[t.range.start..].trim().into();
                    break;
                }
                if t.is("COLLATE") {
                    end = end.min(t.range.start);
                    let start = i + 1;
                    let mut last = start;
                    if top.get(last).is_none() {
                        return Err(invalid("Collation identifier missing"));
                    }
                    while top.get(last + 1).is_some_and(|t| t.is("."))
                        && top.get(last + 2).is_some()
                    {
                        last += 2;
                    }
                    output.collation = part[top[start].range.start..top[last].range.end].into();
                    i = last + 1;
                    continue;
                }
                if t.is("WITH") {
                    end = end.min(t.range.start);
                    output.exclude_operator = part[t.range.end..].trim().into();
                    break;
                }
                if i > 0
                    && !t.quoted
                    && !t.literal
                    && (t.value.ends_with("_ops") || t.value.ends_with("_opclass"))
                {
                    end = end.min(t.range.start);
                    output.operator_class.clone_from(&t.value);
                }
                i += 1;
            }
            output.expression = part[..end].trim().into();
            Ok(output)
        })
        .collect()
}
#[allow(
    clippy::too_many_lines,
    reason = "Preserve native column attributes, including those following a generated expression"
)]
pub(super) fn column(id: u64, sql: &str) -> Result<Element> {
    let ts = tokens(sql)?;
    let first = ts.first().ok_or_else(|| invalid("Empty column"))?;
    let mut e = Element::column(id, first.value.clone());
    e.native_sql = sql.into();
    let mut c = Column::default();
    let words = [
        "NOT",
        "NULL",
        "DEFAULT",
        "COLLATE",
        "PRIMARY",
        "UNIQUE",
        "CHECK",
        "REFERENCES",
        "GENERATED",
        "AS",
        "AUTO_INCREMENT",
        "AUTOINCREMENT",
        "COMMENT",
        "ON",
        "CONSTRAINT",
    ];
    let clauses = ts
        .iter()
        .enumerate()
        .filter(|(i, t)| *i > 0 && t.depth == 0 && words.iter().any(|w| t.is(w)))
        .map(|(i, _)| i)
        .collect::<Vec<_>>();
    let end = clauses
        .first()
        .and_then(|i| ts.get(*i))
        .map_or(sql.len(), |t| t.range.start);
    c.data_type = sql[first.range.end..end].trim().into();
    let mut i = *clauses.first().unwrap_or(&ts.len());
    let mut extras: Vec<String> = Vec::new();
    while let Some(t) = ts.get(i) {
        if t.depth != 0 {
            i += 1;
            continue;
        }
        let start = t.range.start;
        let next = (i + if t.is("DEFAULT") { 2 } else { 1 }..ts.len())
            .find(|j| ts[*j].depth == 0 && words.iter().any(|w| ts[*j].is(w)))
            .unwrap_or(ts.len());
        let end = ts.get(next).map_or(sql.len(), |t| t.range.start);
        if t.is("NOT") && ts.get(i + 1).is_some_and(|t| t.is("NULL")) {
            c.not_null = true;
            i += 2;
            continue;
        }
        if t.is("NULL") {
            i += 1;
            continue;
        }
        if t.is("DEFAULT") {
            c.default_sql = sql[t.range.end..end].trim().into();
        } else if t.is("COLLATE") {
            c.collation = ts.get(i + 1).map_or_else(String::new, |t| t.value.clone());
        } else if t.is("COMMENT") {
            e.comment = ts.get(i + 1).map_or_else(String::new, |t| t.value.clone());
        } else if t.is("AUTO_INCREMENT") || t.is("AUTOINCREMENT") {
            c.auto_increment = true;
        } else if t.is("PRIMARY") {
            c.primary = true;
        } else if t.is("UNIQUE") {
            c.unique = true;
        } else if t.is("AS") || t.is("GENERATED") {
            let rest = &sql[start..];
            if let Some(as_token) = tokens(rest)?.iter().find(|t| t.is("AS")) {
                let after = &rest[as_token.range.end..];
                if after.trim_start().starts_with('(') {
                    let (expression, suffix) = parentheses(after)?;
                    c.generated_sql = expression;
                    c.stored = suffix.to_ascii_uppercase().starts_with("STORED")
                        || suffix.to_ascii_uppercase().starts_with("PERSISTENT");
                    let st = tokens(&suffix)?;
                    let start = st
                        .first()
                        .filter(|t| t.is("STORED") || t.is("VIRTUAL") || t.is("PERSISTENT"))
                        .map_or(0, |t| t.range.end);
                    let trailing = suffix[start..].trim();
                    if !trailing.is_empty() {
                        let attrs = column(0, &format!("x INTEGER {trailing}"))?;
                        e.comment = attrs.comment;
                        if let Properties::Column(attrs) = attrs.data {
                            c.not_null |= attrs.not_null;
                            c.primary |= attrs.primary;
                            c.unique |= attrs.unique;
                            if !attrs.collation.is_empty() {
                                c.collation = attrs.collation;
                            }
                            if !attrs.extra_sql.is_empty() {
                                extras.push(attrs.extra_sql);
                            }
                        }
                    }
                    i = ts.len();
                    continue;
                }
            }
            extras.push(sql[start..].trim().into());
            break;
        } else {
            extras.push(sql[start..end].trim().into());
        }
        i = next;
    }
    c.extra_sql = extras.join(" ");
    e.data = Properties::Column(c);
    Ok(e)
}
#[allow(
    clippy::too_many_lines,
    reason = "Native primary, unique, foreign, check and exclusion clauses share one parser"
)]
pub(super) fn key(id: u64, clause: &str) -> Result<Element> {
    let ts = tokens(clause)?;
    let mut start = 0;
    let mut name = String::new();
    if ts.first().is_some_and(|t| t.is("CONSTRAINT")) {
        name = ts.get(1).map_or_else(String::new, |t| t.value.clone());
        start = 2;
    }
    let first = ts.get(start).ok_or_else(|| invalid("Empty constraint"))?;
    let kind = if first.is("PRIMARY") {
        KeyKind::Primary
    } else if first.is("FOREIGN") {
        KeyKind::Foreign
    } else if first.is("CHECK") {
        KeyKind::Check
    } else if first.is("EXCLUDE") {
        KeyKind::Exclusion
    } else {
        KeyKind::Unique
    };
    let (inside, suffix) = parentheses(&clause[first.range.start..])?;
    let mut k = Key {
        kind,
        ..Key::default()
    };
    if kind == KeyKind::Check {
        k.expression = inside;
    } else {
        k.columns = index_parts(&inside)?;
    }
    k.method = value_after(&ts, "USING");
    let suffix_tokens = tokens(&suffix)?;
    if kind == KeyKind::Foreign {
        k.match_type = value_after(&suffix_tokens, "MATCH");
        if suffix_tokens
            .windows(2)
            .any(|w| w[0].is("NOT") && w[1].is("VALID"))
        {
            k.extra_sql = "NOT VALID".into();
        }
        let reference = value_after(&suffix_tokens, "REFERENCES");
        k.referenced_table = reference;
        let r = suffix_tokens.iter().position(|t| t.is("REFERENCES"));
        if let Some(r) = r
            && suffix_tokens.get(r + 2).is_some_and(|t| t.is("."))
        {
            k.referenced_schema.clone_from(&k.referenced_table);
            k.referenced_table = suffix_tokens
                .get(r + 3)
                .map_or_else(String::new, |t| t.value.clone());
        }
        if suffix.contains('(') {
            let (cols, _) = parentheses(&suffix)?;
            k.referenced_columns = parts(&cols)?
                .into_iter()
                .map(|s| {
                    tokens(&s)
                        .ok()
                        .and_then(|t| t.first().map(|t| t.value.clone()))
                        .unwrap_or(s)
                })
                .collect();
        }
        for action in ["DELETE", "UPDATE"] {
            if let Some(i) = suffix_tokens.iter().position(|t| t.is(action)) {
                let value = suffix_tokens
                    .iter()
                    .skip(i + 1)
                    .take_while(|t| {
                        !["ON", "DEFERRABLE", "NOT", "INITIALLY", "MATCH"]
                            .iter()
                            .any(|w| t.is(w))
                    })
                    .map(|t| t.value.as_str())
                    .collect::<Vec<_>>()
                    .join(" ");
                if action == "DELETE" {
                    k.on_delete = value;
                } else {
                    k.on_update = value;
                }
            }
        }
    } else {
        k.extra_sql = suffix;
    }
    let extras = tokens(&k.extra_sql)?;
    let mut spans = Vec::new();
    let mut at = 0;
    while at < extras.len() {
        let token = &extras[at];
        let following = extras.get(at + 1);
        if token.depth == 0 && token.is("NOT") && following.is_some_and(|t| t.is("DEFERRABLE")) {
            spans.push(token.range.start..extras[at + 1].range.end);
            at += 2;
        } else if token.depth == 0 && token.is("DEFERRABLE") {
            spans.push(token.range.clone());
            at += 1;
        } else if token.depth == 0
            && token.is("INITIALLY")
            && following.is_some_and(|t| t.is("DEFERRED") || t.is("IMMEDIATE"))
        {
            spans.push(token.range.start..extras[at + 1].range.end);
            at += 2;
        } else {
            at += 1;
        }
    }
    for span in spans.into_iter().rev() {
        k.extra_sql.replace_range(span, "");
    }
    k.extra_sql = k.extra_sql.trim().into();
    k.deferrable = suffix_tokens.iter().enumerate().any(|(i, t)| {
        t.is("DEFERRABLE")
            && !i
                .checked_sub(1)
                .and_then(|j| suffix_tokens.get(j))
                .is_some_and(|t| t.is("NOT"))
    });
    k.deferred = suffix_tokens.iter().any(|t| t.is("DEFERRED"));
    if kind == KeyKind::Primary && name.is_empty() {
        name = "PRIMARY".into();
    }
    if name.is_empty() {
        name = ts
            .get(start + 1)
            .filter(|t| !t.is("KEY") && !t.is("INDEX") && !t.is("("))
            .map_or_else(|| format!("constraint_{id}"), |t| t.value.clone());
        if ts
            .get(start + 1)
            .is_some_and(|t| t.is("KEY") || t.is("INDEX"))
        {
            name = ts
                .get(start + 2)
                .filter(|t| !t.is("("))
                .map_or(name, |t| t.value.clone());
        }
    }
    Ok(Element {
        id,
        name,
        comment: String::new(),
        native_sql: clause.into(),
        data: Properties::Key(k),
    })
}
pub(super) fn index(id: u64, sql: &str) -> Result<Element> {
    let ts = tokens(sql)?;
    let at = ts
        .iter()
        .position(|t| t.is("INDEX") || t.is("KEY"))
        .ok_or_else(|| invalid("Missing index keyword"))?;
    let name = ts
        .get(at + 1)
        .ok_or_else(|| invalid("Missing index name"))?
        .value
        .clone();
    let (inside, suffix) = parentheses(sql)?;
    let suffix_ts = tokens(&suffix)?;
    let mut data = Index {
        unique: ts.iter().any(|t| t.is("UNIQUE")),
        columns: index_parts(&inside)?,
        method: value_after(&ts, "USING"),
        ..Index::default()
    };
    if ts.iter().any(|t| t.is("FULLTEXT")) {
        data.method = "FULLTEXT".into();
    }
    if ts.iter().any(|t| t.is("SPATIAL")) {
        data.method = "SPATIAL".into();
    }
    let mut extras = suffix.clone();
    let mut comment = String::new();
    if let Some(t) = suffix_ts.iter().find(|t| t.is("WHERE")) {
        data.condition = suffix[t.range.end..].trim().into();
        extras = suffix[..t.range.start].trim().into();
    }
    if let Some(t) = suffix_ts.iter().find(|t| t.is("INCLUDE")) {
        let tail = &suffix[t.range.end..];
        let (included, after) = parentheses(tail)?;
        data.include_columns = parts(&included)?
            .into_iter()
            .map(|s| {
                tokens(&s)
                    .ok()
                    .and_then(|ts| ts.first().map(|t| t.value.clone()))
                    .unwrap_or(s)
            })
            .collect();
        let end = suffix.len() - after.len();
        extras = format!("{} {}", &suffix[..t.range.start], &suffix[end..]);
        if let Some(at) = extras.to_ascii_uppercase().find("WHERE") {
            extras.truncate(at);
        }
    }
    let ets = tokens(&extras)?;
    if let Some(at) = ets.iter().position(|t| t.is("COMMENT"))
        && let Some(value) = ets.get(at + 1)
    {
        comment.clone_from(&value.value);
        extras = format!(
            "{} {}",
            &extras[..ets[at].range.start],
            &extras[value.range.end..]
        );
    }
    data.extra_sql = extras.trim().into();
    Ok(Element {
        id,
        name,
        comment,
        native_sql: sql.into(),
        data: Properties::Index(data),
    })
}
pub(super) fn trigger(id: u64, name: String, sql: &str) -> Result<Element> {
    parse_trigger(id, name, sql, false)
}
pub(super) fn mysql_trigger(id: u64, name: String, sql: &str, mode: &str) -> Result<Element> {
    parse_trigger(
        id,
        name,
        sql,
        !mode.split(',').any(|s| s == "NO_BACKSLASH_ESCAPES"),
    )
}
#[allow(
    clippy::too_many_lines,
    reason = "Parse native trigger timing, body, definer, transition and constraint clauses together"
)]
fn parse_trigger(id: u64, name: String, sql: &str, backslash: bool) -> Result<Element> {
    let ts = lex(sql, backslash)?;
    let timing = ts
        .iter()
        .position(|t| t.is("BEFORE") || t.is("AFTER") || t.is("INSTEAD"))
        .ok_or_else(|| invalid("Missing trigger timing"))?;
    let on = ts
        .iter()
        .enumerate()
        .skip(timing + 1)
        .find(|(_, t)| t.is("ON"))
        .map(|(i, _)| i)
        .ok_or_else(|| invalid("Missing trigger table"))?;
    let events_start = timing + if ts[timing].is("INSTEAD") { 2 } else { 1 };
    let row_end = ts
        .windows(3)
        .position(|w| w[0].is("FOR") && w[1].is("EACH") && (w[2].is("ROW") || w[2].is("STATEMENT")))
        .map(|i| i + 3);
    let body = row_end
        .map_or_else(
            || {
                ts.iter()
                    .find(|t| t.depth == 0 && (t.is("BEGIN") || t.is("EXECUTE")))
            },
            |at| {
                if ts.get(at).is_some_and(|t| t.is("WHEN")) {
                    ts.iter()
                        .skip(at)
                        .find(|t| t.depth == 0 && (t.is("BEGIN") || t.is("EXECUTE")))
                } else {
                    ts.get(at)
                }
            },
        )
        .ok_or_else(|| invalid("Missing trigger body"))?;
    let mut data = Trigger {
        timing: if ts[timing].is("INSTEAD") {
            "INSTEAD OF".into()
        } else {
            ts[timing].value.to_ascii_uppercase()
        },
        events: sql[ts[events_start].range.start..ts[on].range.start]
            .trim()
            .into(),
        body_sql: sql[body.range.start..].trim().trim_end_matches(';').into(),
        for_each_row: ts.iter().any(|t| t.is("ROW")),
        enabled: "ENABLE".into(),
        ..Trigger::default()
    };
    data.constraint = ts.iter().any(|t| t.depth == 0 && t.is("CONSTRAINT"));
    data.deferrable = ts.iter().enumerate().any(|(i, t)| {
        t.is("DEFERRABLE")
            && !i
                .checked_sub(1)
                .and_then(|j| ts.get(j))
                .is_some_and(|t| t.is("NOT"))
    });
    data.deferred = ts.iter().any(|t| t.is("DEFERRED"));
    if let Some(from) = ts.iter().position(|t| t.is("FROM"))
        && from < row_end.unwrap_or(ts.len())
    {
        let end = ts
            .iter()
            .skip(from + 1)
            .find(|t| t.is("DEFERRABLE") || t.is("NOT") || t.is("INITIALLY") || t.is("FOR"))
            .map_or(body.range.start, |t| t.range.start);
        data.referenced_table = sql[ts[from].range.end..end].trim().into();
    }
    if let Some(reference) = ts.iter().find(|t| t.is("REFERENCING")) {
        let end = ts
            .iter()
            .find(|t| t.range.start > reference.range.start && (t.is("FOR") || t.is("WHEN")))
            .map_or(body.range.start, |t| t.range.start);
        data.extra_sql = sql[reference.range.start..end].trim().into();
    }
    if let Some(definer) = ts.iter().find(|t| t.is("DEFINER"))
        && let Some(trigger) = ts.iter().find(|t| t.is("TRIGGER"))
    {
        data.definer_sql = sql[definer.range.end..trigger.range.start]
            .trim()
            .trim_start_matches('=')
            .trim()
            .into();
    }
    if let Some(at) = data.events.to_ascii_uppercase().find("UPDATE OF ") {
        let list = data.events[at + 10..].trim().to_owned();
        data.update_columns = parts(&list)?
            .into_iter()
            .map(|s| {
                tokens(&s)
                    .ok()
                    .and_then(|ts| ts.first().map(|t| t.value.clone()))
                    .unwrap_or(s)
            })
            .collect();
        data.events = "UPDATE".into();
    }
    if let Some(when) = ts.iter().find(|t| t.is("WHEN")) {
        let raw = sql[when.range.end..body.range.start].trim();
        data.condition = if raw.starts_with('(') {
            parentheses(raw)?.0
        } else {
            raw.into()
        };
    }
    Ok(Element {
        id,
        name,
        comment: String::new(),
        data: Properties::Trigger(data),
        native_sql: sql.trim_end_matches(';').into(),
    })
}
