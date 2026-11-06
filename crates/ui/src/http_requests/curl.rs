//! `cURL` text for one saved request. The builder and this text describe the same call.
#![allow(clippy::redundant_pub_crate)]

use super::model::{
    BodyKind, Field, Request, absorb_url_query, content_type_kind, parse_query, percent_decode,
    percent_encode,
};

/// Fields taken from a `cURL` command. Name, tags and id stay on the saved request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ParsedCurl {
    pub method: String,
    pub url: String,
    pub query: Vec<Field>,
    pub headers: Vec<Field>,
    pub body_kind: BodyKind,
    pub body: String,
    pub form: Vec<Field>,
}

pub(crate) fn render(request: &Request) -> String {
    let mut parts = vec![format!(
        "curl -X {} {}",
        request.method,
        shell_quote(&url_with_query(&request.url, &request.query))
    )];
    for field in request
        .headers
        .iter()
        .filter(|field| field.enabled && !field.name.is_empty())
    {
        parts.push(format!(
            "-H {}",
            shell_quote(&format!("{}: {}", field.name, field.value))
        ));
    }
    match request.body_kind {
        BodyKind::None => {}
        BodyKind::Json | BodyKind::Text => {
            parts.push(format!("--data-raw {}", shell_quote(&request.body)));
        }
        BodyKind::Form => {
            for field in enabled_fields(&request.form) {
                parts.push(format!(
                    "--data-urlencode {}",
                    shell_quote(&format!("{}={}", field.name, field.value))
                ));
            }
        }
        BodyKind::Multipart => {
            for field in enabled_fields(&request.form) {
                parts.push(format!(
                    "-F {}",
                    shell_quote(&format!("{}={}", field.name, field.value))
                ));
            }
        }
    }
    if parts.len() == 1 {
        parts.remove(0)
    } else {
        parts.join(" \\\n  ")
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "Option decoding stays in one pass so mixed body flags are rejected together"
)]
pub(crate) fn parse(text: &str) -> Result<ParsedCurl, String> {
    if text.len() > 1024 * 1024 {
        return Err("cURL text is larger than 1 MiB".into());
    }
    let tokens = tokenize(text)?;
    if tokens.is_empty() {
        return Err("Enter a curl command".into());
    }
    let command = tokens[0].as_str();
    if command != "curl" && command != "curl.exe" {
        return Err("cURL text must start with curl".into());
    }
    let mut method = String::new();
    let mut method_set = false;
    let mut url = String::new();
    let mut headers = Vec::new();
    let mut data_parts = Vec::new();
    let mut urlencode_parts = Vec::new();
    let mut form_parts = Vec::new();
    let mut get = false;
    let mut index = 1;
    while index < tokens.len() {
        let token = &tokens[index];
        if token == "--" {
            index += 1;
            if index < tokens.len() {
                take_url(&mut url, &tokens[index])?;
                index += 1;
            }
            continue;
        }
        if let Some(option) = token.strip_prefix("--") {
            let (name, inline) = option
                .split_once('=')
                .map_or((option, None), |(name, value)| (name, Some(value)));
            match name {
                "request" => {
                    method = value_of(&tokens, &mut index, inline)?.to_ascii_uppercase();
                    method_set = true;
                }
                "url" => take_url(&mut url, &value_of(&tokens, &mut index, inline)?)?,
                "header" => headers.push(parse_header(&value_of(&tokens, &mut index, inline)?)?),
                "data" | "data-raw" | "data-binary" | "data-ascii" => {
                    data_parts.push(value_of(&tokens, &mut index, inline)?);
                }
                "data-urlencode" => urlencode_parts.push(value_of(&tokens, &mut index, inline)?),
                "form" | "form-string" => form_parts.push(value_of(&tokens, &mut index, inline)?),
                "get" => get = true,
                "json" => {
                    headers.push(Field::new("Content-Type", "application/json"));
                    headers.push(Field::new("Accept", "application/json"));
                    data_parts.push(value_of(&tokens, &mut index, inline)?);
                }
                "user-agent" => {
                    headers.push(Field::new(
                        "User-Agent",
                        value_of(&tokens, &mut index, inline)?,
                    ));
                }
                "referer" => {
                    headers.push(Field::new(
                        "Referer",
                        value_of(&tokens, &mut index, inline)?,
                    ));
                }
                "cookie" => {
                    headers.push(Field::new("Cookie", value_of(&tokens, &mut index, inline)?));
                }
                "user" => headers.push(basic_auth(&value_of(&tokens, &mut index, inline)?)),
                _ if ignored_value(name) => {
                    let _ignored = value_of(&tokens, &mut index, inline)?;
                }
                _ if ignored_flag(name) => {}
                _ => return Err(format!("Unsupported curl option --{name}")),
            }
            index += 1;
            continue;
        }
        if token.starts_with('-') && token != "-" {
            index = short_option(
                &tokens,
                index,
                &mut method,
                &mut method_set,
                &mut url,
                &mut headers,
                &mut data_parts,
                &mut urlencode_parts,
                &mut form_parts,
                &mut get,
            )?;
            continue;
        }
        take_url(&mut url, token)?;
        index += 1;
    }
    if !form_parts.is_empty() && (!data_parts.is_empty() || !urlencode_parts.is_empty()) {
        return Err("Use either --form or --data, not both".into());
    }
    if url.is_empty() {
        return Err("cURL command has no URL".into());
    }
    let (url, mut query) = split_url(url);
    if get {
        for part in &data_parts {
            query.extend(parse_query(part));
        }
        for part in &urlencode_parts {
            query.push(urlencode_field(part));
        }
        data_parts.clear();
        urlencode_parts.clear();
    }
    let (body_kind, body, form) = decode_body(&headers, &data_parts, &urlencode_parts, &form_parts);
    if !method_set {
        method = if matches!(body_kind, BodyKind::None) {
            "GET".into()
        } else {
            "POST".into()
        };
    }
    Ok(ParsedCurl {
        method,
        url,
        query,
        headers,
        body_kind,
        body,
        form,
    })
}

pub(crate) fn apply(request: &mut Request, parsed: ParsedCurl) {
    request.method = parsed.method;
    request.url = parsed.url;
    request.query = parsed.query;
    request.headers = parsed.headers;
    request.body_kind = parsed.body_kind;
    request.body = parsed.body;
    request.form = parsed.form;
    absorb_url_query(request);
}

fn decode_body(
    headers: &[Field],
    data_parts: &[String],
    urlencode_parts: &[String],
    form_parts: &[String],
) -> (BodyKind, String, Vec<Field>) {
    if !form_parts.is_empty() {
        return (
            BodyKind::Multipart,
            String::new(),
            form_parts.iter().map(|part| form_field(part)).collect(),
        );
    }
    if !urlencode_parts.is_empty() && data_parts.is_empty() {
        return (
            BodyKind::Form,
            String::new(),
            urlencode_parts
                .iter()
                .map(|part| urlencode_field(part))
                .collect(),
        );
    }
    if data_parts.is_empty() && urlencode_parts.is_empty() {
        return (BodyKind::None, String::new(), Vec::new());
    }
    let mut body = data_parts.join("&");
    if !urlencode_parts.is_empty() {
        if !body.is_empty() {
            body.push('&');
        }
        body.push_str(
            &urlencode_parts
                .iter()
                .map(|part| {
                    let field = urlencode_field(part);
                    format!(
                        "{}={}",
                        percent_encode(&field.name),
                        percent_encode(&field.value)
                    )
                })
                .collect::<Vec<_>>()
                .join("&"),
        );
    }
    let content_type = headers.iter().rev().find_map(|field| {
        field
            .name
            .eq_ignore_ascii_case("content-type")
            .then_some(field.value.as_str())
    });
    if content_type.is_some_and(|value| content_type_kind(value) == Some(BodyKind::Form)) {
        return (BodyKind::Form, String::new(), form_body_fields(&body));
    }
    if content_type.is_some_and(|value| content_type_kind(value) == Some(BodyKind::Multipart))
        && let Some(fields) = json_object_fields(&body)
    {
        return (BodyKind::Multipart, String::new(), fields);
    }
    let kind = content_type
        .and_then(content_type_kind)
        .filter(|kind| !matches!(kind, BodyKind::Form | BodyKind::Multipart))
        .or_else(|| json_body(&body).then_some(BodyKind::Json))
        .unwrap_or(BodyKind::Text);
    (kind, body, Vec::new())
}

fn json_body(body: &str) -> bool {
    let trimmed = body.trim_start();
    (trimmed.starts_with('{') || trimmed.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(body).is_ok()
}

fn json_object_fields(body: &str) -> Option<Vec<Field>> {
    let value: serde_json::Value = serde_json::from_str(body).ok()?;
    let map = value.as_object()?;
    Some(
        map.iter()
            .map(|(name, item)| Field::new(name.clone(), scalar(item)))
            .collect(),
    )
}

fn scalar(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) => number.to_string(),
        serde_json::Value::Bool(flag) => flag.to_string(),
        serde_json::Value::Null => String::new(),
        other => other.to_string(),
    }
}

fn form_body_fields(body: &str) -> Vec<Field> {
    if let Some(fields) = json_object_fields(body) {
        return fields;
    }
    body.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((name, value)) => {
                Field::new(percent_decode(name, true), percent_decode(value, true))
            }
            None => Field::new(percent_decode(part, true), String::new()),
        })
        .collect()
}

fn form_field(part: &str) -> Field {
    match part.split_once('=') {
        Some((name, value)) => Field::new(name, value),
        None => Field::new(part, String::new()),
    }
}

fn urlencode_field(part: &str) -> Field {
    match part.split_once('=') {
        Some((name, value)) => Field::new(percent_decode(name, true), percent_decode(value, true)),
        None => Field::new(percent_decode(part, true), String::new()),
    }
}

fn split_url(url: String) -> (String, Vec<Field>) {
    let (before_fragment, fragment) = match url.split_once('#') {
        Some((base, fragment)) => (base.to_owned(), Some(fragment.to_owned())),
        None => (url, None),
    };
    let (base, query) = match before_fragment.split_once('?') {
        Some((base, query)) => (base.to_owned(), parse_query(query)),
        None => (before_fragment, Vec::new()),
    };
    let url = match fragment {
        Some(fragment) => format!("{base}#{fragment}"),
        None => base,
    };
    (url, query)
}

fn take_url(url: &mut String, next: &str) -> Result<(), String> {
    if !url.is_empty() {
        return Err("cURL command has more than one URL".into());
    }
    next.clone_into(url);
    Ok(())
}

fn parse_header(text: &str) -> Result<Field, String> {
    let Some((name, value)) = text.split_once(':') else {
        return Err("A curl header must look like Name: value".into());
    };
    let name = name.trim();
    if name.is_empty() || name.ends_with(';') {
        return Err("A curl header must look like Name: value".into());
    }
    Ok(Field::new(name, value.trim()))
}

fn basic_auth(raw: &str) -> Field {
    use base64::Engine;
    let (user, password) = raw.split_once(':').unwrap_or((raw, ""));
    let encoded = base64::engine::general_purpose::STANDARD.encode(format!("{user}:{password}"));
    Field::new("Authorization", format!("Basic {encoded}"))
}

fn value_of(tokens: &[String], index: &mut usize, inline: Option<&str>) -> Result<String, String> {
    if let Some(value) = inline {
        return Ok(value.to_owned());
    }
    let next = tokens
        .get(*index + 1)
        .ok_or_else(|| "A curl option is missing its value".to_string())?;
    *index += 1;
    Ok(next.clone())
}

#[allow(
    clippy::too_many_arguments,
    reason = "Short options update every part of the command the long options already collect"
)]
fn short_option(
    tokens: &[String],
    mut index: usize,
    method: &mut String,
    method_set: &mut bool,
    _url: &mut String,
    headers: &mut Vec<Field>,
    data_parts: &mut Vec<String>,
    _urlencode_parts: &mut Vec<String>,
    form_parts: &mut Vec<String>,
    get: &mut bool,
) -> Result<usize, String> {
    let cluster: Vec<char> = tokens[index].chars().skip(1).collect();
    let mut cursor = 0;
    while cursor < cluster.len() {
        let flag = cluster[cursor];
        let rest: String = cluster[cursor + 1..].iter().collect();
        let inline = if rest.is_empty() { None } else { Some(rest) };
        match flag {
            'X' => {
                *method = value_of(tokens, &mut index, inline.as_deref())?.to_ascii_uppercase();
                *method_set = true;
                break;
            }
            'H' => {
                headers.push(parse_header(&value_of(
                    tokens,
                    &mut index,
                    inline.as_deref(),
                )?)?);
                break;
            }
            'd' => {
                data_parts.push(value_of(tokens, &mut index, inline.as_deref())?);
                break;
            }
            'F' => {
                form_parts.push(value_of(tokens, &mut index, inline.as_deref())?);
                break;
            }
            'G' => *get = true,
            'A' => {
                headers.push(Field::new(
                    "User-Agent",
                    value_of(tokens, &mut index, inline.as_deref())?,
                ));
                break;
            }
            'e' => {
                headers.push(Field::new(
                    "Referer",
                    value_of(tokens, &mut index, inline.as_deref())?,
                ));
                break;
            }
            'b' => {
                headers.push(Field::new(
                    "Cookie",
                    value_of(tokens, &mut index, inline.as_deref())?,
                ));
                break;
            }
            'u' => {
                headers.push(basic_auth(&value_of(
                    tokens,
                    &mut index,
                    inline.as_deref(),
                )?));
                break;
            }
            's' | 'S' | 'v' | 'k' | 'L' | 'i' | 'f' => {}
            'o' | 'm' | 'w' | 'x' => {
                let _ignored = value_of(tokens, &mut index, inline.as_deref())?;
                break;
            }
            other => return Err(format!("Unsupported curl option -{other}")),
        }
        cursor += 1;
    }
    Ok(index + 1)
}

fn ignored_flag(name: &str) -> bool {
    matches!(
        name,
        "silent"
            | "show-error"
            | "verbose"
            | "insecure"
            | "location"
            | "include"
            | "fail"
            | "compressed"
            | "http1.0"
            | "http1.1"
            | "http2"
            | "http2-prior-knowledge"
            | "path-as-is"
            | "globoff"
            | "raw"
            | "no-buffer"
    )
}

fn ignored_value(name: &str) -> bool {
    matches!(
        name,
        "connect-timeout"
            | "max-time"
            | "retry"
            | "retry-delay"
            | "retry-max-time"
            | "output"
            | "write-out"
            | "proxy"
            | "proxy-user"
            | "config"
            | "keepalive-time"
            | "speed-limit"
            | "speed-time"
    )
}

fn enabled_fields(fields: &[Field]) -> impl Iterator<Item = &Field> {
    fields
        .iter()
        .filter(|field| field.enabled && !field.name.is_empty())
}

fn url_with_query(url: &str, query: &[Field]) -> String {
    let rows: Vec<_> = enabled_fields(query).collect();
    if rows.is_empty() {
        return url.to_owned();
    }
    let (base, fragment) = match url.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (url, None),
    };
    let mut out = base.to_owned();
    out.push('?');
    for (index, field) in rows.iter().enumerate() {
        if index > 0 {
            out.push('&');
        }
        out.push_str(&percent_encode(&field.name));
        out.push('=');
        out.push_str(&percent_encode(&field.value));
    }
    if let Some(fragment) = fragment {
        out.push('#');
        out.push_str(fragment);
    }
    out
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn tokenize(input: &str) -> Result<Vec<String>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut index = 0;
    let mut tokens = Vec::new();
    while index < chars.len() {
        index = skip_separator(&chars, index);
        if index >= chars.len() {
            break;
        }
        let mut word = String::new();
        let mut any = false;
        while index < chars.len() && !chars[index].is_whitespace() {
            any = true;
            if chars[index] == '\\'
                && index + 1 < chars.len()
                && (chars[index + 1] == '\n' || chars[index + 1] == '\r')
            {
                break;
            }
            if chars[index] == '\'' {
                index += 1;
                while index < chars.len() && chars[index] != '\'' {
                    word.push(chars[index]);
                    index += 1;
                }
                if index >= chars.len() {
                    return Err("Unclosed single quote in curl".into());
                }
            } else if chars[index] == '"' {
                index += 1;
                while index < chars.len() && chars[index] != '"' {
                    if chars[index] == '\\' && index + 1 < chars.len() {
                        index += 1;
                        word.push(match chars[index] {
                            'n' => '\n',
                            't' => '\t',
                            'r' => '\r',
                            other => other,
                        });
                    } else {
                        word.push(chars[index]);
                    }
                    index += 1;
                }
                if index >= chars.len() {
                    return Err("Unclosed double quote in curl".into());
                }
            } else {
                if chars[index] == '\\' && index + 1 < chars.len() {
                    index += 1;
                }
                word.push(chars[index]);
            }
            index += 1;
        }
        if any {
            tokens.push(word);
        }
    }
    Ok(tokens)
}

const fn skip_separator(chars: &[char], mut index: usize) -> usize {
    while index < chars.len() {
        if chars[index] == '\\' && index + 1 < chars.len() && chars[index + 1] == '\r' {
            index += 2;
            if index < chars.len() && chars[index] == '\n' {
                index += 1;
            }
            continue;
        }
        if chars[index] == '\\' && index + 1 < chars.len() && chars[index + 1] == '\n' {
            index += 2;
            continue;
        }
        if chars[index].is_whitespace() {
            index += 1;
            continue;
        }
        break;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request() -> Request {
        Request::blank("h1".into(), 1)
    }

    #[test]
    fn builder_and_curl_round_trip_get_post_form_and_multipart() -> Result<(), String> {
        let mut get = request();
        get.url = "https://example.com/users".into();
        get.query = vec![Field::new("page", "2"), {
            let mut hidden = Field::new("debug", "1");
            hidden.enabled = false;
            hidden
        }];
        get.headers = vec![Field::new("Accept", "application/json")];
        let parsed = parse(&render(&get))?;
        assert_eq!(parsed.method, "GET");
        assert_eq!(parsed.url, "https://example.com/users");
        assert_eq!(parsed.query, vec![Field::new("page", "2")]);
        assert_eq!(
            parsed.headers,
            vec![Field::new("Accept", "application/json")]
        );
        assert_eq!(parsed.body_kind, BodyKind::None);

        let mut post = request();
        post.method = "POST".into();
        post.url = "https://example.com/users".into();
        post.body_kind = BodyKind::Json;
        post.body = "{\"name\":\"Ada\"}".into();
        post.headers = vec![Field::new("Content-Type", "application/json")];
        let parsed = parse(&render(&post))?;
        assert_eq!(parsed.method, "POST");
        assert_eq!(parsed.body_kind, BodyKind::Json);
        assert_eq!(parsed.body, "{\"name\":\"Ada\"}");

        let mut form = request();
        form.method = "POST".into();
        form.url = "https://example.com/login".into();
        form.body_kind = BodyKind::Form;
        form.form = vec![Field::new("user", "ada"), Field::new("password", "secret")];
        form.headers = vec![Field::new(
            "Content-Type",
            "application/x-www-form-urlencoded",
        )];
        let parsed = parse(&render(&form))?;
        assert_eq!(parsed.body_kind, BodyKind::Form);
        assert_eq!(parsed.form, form.form);

        let mut multipart = request();
        multipart.method = "POST".into();
        multipart.url = "https://example.com/upload".into();
        multipart.body_kind = BodyKind::Multipart;
        multipart.form = vec![Field::new("title", "Hello"), Field::new("note", "line")];
        let parsed = parse(&render(&multipart))?;
        assert_eq!(parsed.body_kind, BodyKind::Multipart);
        assert_eq!(parsed.form, multipart.form);
        Ok(())
    }

    #[test]
    fn parses_continuations_quotes_get_and_rejects_unknown_options() -> Result<(), String> {
        let parsed = parse(
            "curl -X GET \\\n  'https://example.com/search?q=rust' \\\n  -H 'Accept: application/json'",
        )?;
        assert_eq!(parsed.url, "https://example.com/search");
        assert_eq!(parsed.query, vec![Field::new("q", "rust")]);
        let quoted = parse(
            "curl -X POST 'https://example.com' -H 'X-Name: a'\\''b' --data-raw 'say \"hi\"'",
        )?;
        assert_eq!(quoted.headers, vec![Field::new("X-Name", "a'b")]);
        assert_eq!(quoted.body, "say \"hi\"");
        let moved = parse("curl -G 'https://example.com/search' --data-urlencode 'q=rust'")?;
        assert_eq!(moved.method, "GET");
        assert_eq!(moved.query, vec![Field::new("q", "rust")]);
        assert_eq!(moved.body_kind, BodyKind::None);
        assert!(parse("curl --junk https://example.com").is_err());
        Ok(())
    }

    #[test]
    fn applying_curl_keeps_name_and_tags() -> Result<(), String> {
        let mut saved = request();
        saved.name = "List".into();
        saved.tags = vec!["users".into()];
        apply(
            &mut saved,
            parse("curl -X DELETE 'https://example.com/users/7'")?,
        );
        assert_eq!(saved.name, "List");
        assert_eq!(saved.tags, vec!["users".to_owned()]);
        assert_eq!(saved.method, "DELETE");
        assert_eq!(saved.url, "https://example.com/users/7");
        Ok(())
    }
}
