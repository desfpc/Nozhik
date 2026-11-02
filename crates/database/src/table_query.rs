//! Server-side table selection, independent of retained page rows.
use crate::{DbDriverType, Error, Result, connection::quote_identifier, introspection::TableInfo};

/// SQL expressions entered in a table's WHERE and ORDER BY fields, without keywords.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TableQuery {
    /// Optional predicate, e.g. `id < 1000`.
    pub predicate: String,
    /// Optional order list, e.g. `name ASC, id DESC`.
    pub order: String,
}
impl TableQuery {
    /// Build one bounded SELECT. Empty ordering leaves the server order unchanged.
    /// # Errors
    /// Rejects statement separators, SQL comments and top-level clause escapes.
    pub fn page_sql(
        &self,
        driver: DbDriverType,
        schema: &str,
        table: &TableInfo,
        offset: usize,
        size: usize,
    ) -> Result<String> {
        validate(&self.predicate, driver)?;
        validate(&self.order, driver)?;
        let predicate = self.predicate.trim();
        let order = self.order.trim();
        Ok(format!(
            "SELECT * FROM {}.{}{}{} LIMIT {} OFFSET {offset}",
            quote_identifier(driver, schema),
            quote_identifier(driver, &table.name),
            if predicate.is_empty() {
                String::new()
            } else {
                format!(" WHERE ({predicate})")
            },
            if order.is_empty() {
                String::new()
            } else {
                format!(" ORDER BY {order}")
            },
            size.clamp(1, 5000) + 1
        ))
    }
    /// Recognize a single column ordering for the header's three-state control.
    /// Complex manual expressions keep neutral header arrows.
    #[must_use]
    pub fn column_descending(&self, driver: DbDriverType, name: &str) -> Option<bool> {
        let order = self.order.trim();
        let upper = order.to_ascii_uppercase();
        let (column, descending) = if upper.ends_with(" DESC") {
            (order.get(..order.len() - 5)?.trim_end(), true)
        } else if upper.ends_with(" ASC") {
            (order.get(..order.len() - 4)?.trim_end(), false)
        } else {
            (order, false)
        };
        (column == quote_identifier(driver, name) || column == name).then_some(descending)
    }
}

// These fields contain expressions, not scripts. Quoted SQL values/identifiers
// may contain punctuation; standard doubled quotes are supported. Reject ambiguous
// backslash escapes instead of guessing a connection's current string escape mode.
fn validate(source: &str, driver: DbDriverType) -> Result<()> {
    let invalid = || {
        Error::Invalid("WHERE / ORDER BY require SQL expressions without statement separators, comments or extra clauses".into())
    };
    if source.len() > 16 * 1024 {
        return Err(invalid());
    }
    let mut quote = None;
    let mut depth = 0_usize;
    let mut word = String::new();
    let check = |word: &str, depth| -> Result<()> {
        if depth == 0
            && matches!(
                word.to_ascii_uppercase().as_str(),
                "SELECT"
                    | "FROM"
                    | "WHERE"
                    | "ORDER"
                    | "GROUP"
                    | "HAVING"
                    | "LIMIT"
                    | "OFFSET"
                    | "UNION"
                    | "INTERSECT"
                    | "EXCEPT"
                    | "INTO"
                    | "FOR"
                    | "LOCK"
                    | "FETCH"
                    | "RETURNING"
            )
        {
            return Err(invalid());
        }
        Ok(())
    };
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        if let Some(end) = quote {
            if c == '\\' {
                return Err(Error::Invalid(
                    "Use doubled SQL quotes rather than backslash escapes in table conditions"
                        .into(),
                ));
            }
            if c == end {
                if chars.peek() == Some(&end) {
                    chars.next();
                } else {
                    quote = None;
                }
            }
            continue;
        }
        if c == ';'
            || c == '\0'
            || (c == '#' && driver == DbDriverType::Mysql)
            || (c == '-' && chars.peek() == Some(&'-'))
            || (c == '/' && chars.peek() == Some(&'*'))
        {
            return Err(invalid());
        }
        if c.is_alphanumeric() || c == '_' {
            word.push(c);
            continue;
        }
        check(&word, depth)?;
        word.clear();
        match c {
            '\'' | '"' | '`' => quote = Some(c),
            '[' => quote = Some(']'),
            '(' => depth += 1,
            ')' => depth = depth.checked_sub(1).ok_or_else(invalid)?,
            _ => {}
        }
    }
    check(&word, depth)?;
    if quote.is_some() || depth != 0 {
        return Err(invalid());
    }
    Ok(())
}
