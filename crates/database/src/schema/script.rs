//! MySQL client DELIMITER handling outside strings and comments.
use crate::{DbDriverType, Error, Result};
#[derive(Default, PartialEq, Eq)]
enum State {
    #[default]
    Normal,
    Quote(u8),
    Block,
    Line,
}
pub(super) fn script_batches(sql: &str, driver: DbDriverType) -> Result<Vec<String>> {
    if driver != DbDriverType::Mysql {
        return Ok(vec![sql.into()]);
    }
    let mut delimiter = ";".to_owned();
    let mut buffer = String::new();
    let mut batches = Vec::new();
    let mut state = State::Normal;
    let mut no_backslash = false;
    for line in sql.split_inclusive('\n') {
        if state == State::Normal {
            let trimmed = line.trim();
            if trimmed.to_ascii_uppercase().starts_with("SET SQL_MODE='") {
                no_backslash = trimmed
                    .to_ascii_uppercase()
                    .contains("NO_BACKSLASH_ESCAPES");
            }
            let mut parts = trimmed.split_whitespace();
            if parts
                .next()
                .is_some_and(|s| s.eq_ignore_ascii_case("DELIMITER"))
            {
                let next = parts
                    .next()
                    .ok_or_else(|| Error::Invalid("DELIMITER requires a token".into()))?;
                if parts.next().is_some() || next.len() > 64 {
                    return Err(Error::Invalid(
                        "DELIMITER must be one token of at most 64 bytes".into(),
                    ));
                }
                if !buffer.trim().is_empty() {
                    batches.push(std::mem::take(&mut buffer));
                }
                delimiter = next.into();
                continue;
            }
        }
        let bytes = line.as_bytes();
        let mut start = 0;
        let mut i = 0;
        while let Some(&byte) = bytes.get(i) {
            match state {
                State::Normal => {
                    if delimiter != ";"
                        && bytes
                            .get(i..)
                            .is_some_and(|s| s.starts_with(delimiter.as_bytes()))
                    {
                        buffer.push_str(&line[start..i]);
                        if !buffer.trim().is_empty() {
                            batches.push(std::mem::take(&mut buffer));
                        }
                        i += delimiter.len();
                        start = i;
                        continue;
                    }
                    if b"'\"`".contains(&byte) {
                        state = State::Quote(byte);
                    } else if byte == b'#'
                        || (byte == b'-'
                            && bytes.get(i + 1) == Some(&b'-')
                            && bytes.get(i + 2).is_none_or(u8::is_ascii_whitespace))
                    {
                        state = State::Line;
                    } else if byte == b'/' && bytes.get(i + 1) == Some(&b'*') {
                        state = State::Block;
                        i += 1;
                    }
                }
                State::Quote(quote) => {
                    if byte == b'\\' && !no_backslash {
                        i += 1;
                    } else if byte == quote {
                        if bytes.get(i + 1) == Some(&quote) {
                            i += 1;
                        } else {
                            state = State::Normal;
                        }
                    }
                }
                State::Block => {
                    if byte == b'*' && bytes.get(i + 1) == Some(&b'/') {
                        state = State::Normal;
                        i += 1;
                    }
                }
                State::Line => {}
            }
            i += 1;
        }
        buffer.push_str(&line[start..]);
        if state == State::Line {
            state = State::Normal;
        }
    }
    if !buffer.trim().is_empty() {
        batches.push(buffer);
    }
    Ok(batches)
}
