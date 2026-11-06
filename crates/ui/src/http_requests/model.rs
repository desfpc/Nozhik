//! Saved HTTP calls stored in `<project>/http_requests.json`.
#![allow(clippy::redundant_pub_crate)]

use serde::{Deserialize, Serialize};

pub(crate) const FILE_NAME: &str = "http_requests.json";
pub(crate) const MAX_REQUESTS: usize = 2_000;
pub(crate) const MAX_BODY_BYTES: usize = 256 * 1024;
pub(crate) const MAX_NAME_CHARS: usize = 200;
pub(crate) const MAX_URL_BYTES: usize = 8 * 1024;
pub(crate) const MAX_FIELDS: usize = 100;
pub(crate) const MAX_FIELD_CHARS: usize = 8_192;
pub(crate) const MAX_TAGS: usize = 64;
pub(crate) const MAX_TAG_CHARS: usize = 40;

/// How the request body is encoded on the wire and in the `cURL` pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BodyKind {
    /// No body is sent.
    #[default]
    None,
    /// Raw text sent as `application/json` unless a `Content-Type` header overrides it.
    Json,
    /// Raw text sent as `text/plain` unless a `Content-Type` header overrides it.
    Text,
    /// Enabled rows sent as `application/x-www-form-urlencoded`.
    Form,
    /// Enabled rows sent as `multipart/form-data`.
    Multipart,
}

impl BodyKind {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::None => "No body",
            Self::Json => "JSON",
            Self::Text => "Text",
            Self::Form => "Form URL encoded",
            Self::Multipart => "Multipart form-data",
        }
    }

    /// Content type implied by the body kind. Multipart sets its own boundary at send time.
    pub(crate) const fn default_content_type(self) -> Option<&'static str> {
        match self {
            Self::None | Self::Multipart => None,
            Self::Json => Some("application/json"),
            Self::Text => Some("text/plain"),
            Self::Form => Some("application/x-www-form-urlencoded"),
        }
    }
}

/// One header, query parameter or form field.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Field {
    /// Field name. Empty names are kept in the editor and omitted from the wire.
    pub name: String,
    /// Field value.
    pub value: String,
    /// When false, the row stays in the builder and is left out of `cURL` and the call.
    #[serde(default = "enabled_by_default")]
    pub enabled: bool,
}

impl Field {
    pub(crate) fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }
}

const fn enabled_by_default() -> bool {
    true
}

/// One saved call. Identity, name and tags survive a `cURL` edit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Request {
    /// Stable id inside `http_requests.json`.
    pub id: String,
    /// List label and `OpenAPI` summary.
    pub name: String,
    /// HTTP method, stored in upper case.
    pub method: String,
    /// URL without the query string. The query lives in [`Request::query`].
    pub url: String,
    /// Query parameters, including for methods that also send a body.
    #[serde(default)]
    pub query: Vec<Field>,
    /// Request headers. `Content-Type` is kept here when the user or the body kind sets it.
    #[serde(default)]
    pub headers: Vec<Field>,
    /// Body encoding.
    #[serde(default)]
    pub body_kind: BodyKind,
    /// Raw JSON or text body. Ignored for form kinds.
    #[serde(default)]
    pub body: String,
    /// Form and multipart rows.
    #[serde(default)]
    pub form: Vec<Field>,
    /// Tags shared with the sidebar, in file order.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Unix milliseconds when the request was created.
    pub created_ms: i64,
    /// Unix milliseconds when the request last changed.
    pub updated_ms: i64,
}

impl Request {
    pub(crate) fn blank(id: String, now: i64) -> Self {
        Self {
            id,
            name: "Untitled request".into(),
            method: "GET".into(),
            url: String::new(),
            query: Vec::new(),
            headers: Vec::new(),
            body_kind: BodyKind::None,
            body: String::new(),
            form: Vec::new(),
            tags: Vec::new(),
            created_ms: now,
            updated_ms: now,
        }
    }
}

/// A call captured from an `OpenAPI` operation, before it receives an id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Incoming {
    /// Suggested list name.
    pub name: String,
    /// HTTP method.
    pub method: String,
    /// URL without the query string.
    pub url: String,
    /// Query parameters.
    pub query: Vec<(String, String)>,
    /// Headers.
    pub headers: Vec<(String, String)>,
    /// Body encoding.
    pub body_kind: BodyKind,
    /// Raw body for JSON and text.
    pub body: String,
    /// Form rows.
    pub form: Vec<(String, String)>,
    /// Operation tags.
    pub tags: Vec<String>,
}

impl Incoming {
    pub(crate) fn into_request(self, id: String, now: i64) -> Result<Request, String> {
        let mut request = Request {
            id,
            name: self.name,
            method: self.method,
            url: self.url,
            query: self
                .query
                .into_iter()
                .map(|(name, value)| Field::new(name, value))
                .collect(),
            headers: self
                .headers
                .into_iter()
                .map(|(name, value)| Field::new(name, value))
                .collect(),
            body_kind: self.body_kind,
            body: self.body,
            form: self
                .form
                .into_iter()
                .map(|(name, value)| Field::new(name, value))
                .collect(),
            tags: self.tags,
            created_ms: now,
            updated_ms: now,
        };
        normalize_request(&mut request)?;
        Ok(request)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub(crate) struct Store {
    #[serde(default)]
    pub requests: Vec<Request>,
}

pub(crate) fn normalize_request(request: &mut Request) -> Result<(), String> {
    request.method = request.method.trim().to_ascii_uppercase();
    if request.method.is_empty()
        || !request
            .method
            .chars()
            .all(|symbol| symbol.is_ascii_alphabetic())
    {
        return Err("HTTP method must be letters".into());
    }
    limit_chars(&mut request.name, MAX_NAME_CHARS);
    if request.name.chars().any(char::is_control) {
        return Err("Request name contains a control character".into());
    }
    if request.name.trim().is_empty() {
        request.name = "Untitled request".into();
    }
    if request.url.len() > MAX_URL_BYTES {
        return Err("URL is longer than 8 KiB".into());
    }
    if request.body.len() > MAX_BODY_BYTES {
        return Err("Body is larger than 256 KiB".into());
    }
    normalize_fields(&mut request.query)?;
    normalize_fields(&mut request.headers)?;
    normalize_fields(&mut request.form)?;
    request.tags = normalize_tags(&request.tags)?;
    if request.created_ms < 0 || request.updated_ms < 0 {
        return Err("http_requests.json has an invalid timestamp".into());
    }
    Ok(())
}

fn normalize_fields(fields: &mut Vec<Field>) -> Result<(), String> {
    if fields.len() > MAX_FIELDS {
        return Err(format!("A request has more than {MAX_FIELDS} fields"));
    }
    for field in fields {
        limit_chars(&mut field.name, MAX_FIELD_CHARS);
        limit_chars(&mut field.value, MAX_FIELD_CHARS);
        if field.name.chars().any(char::is_control) || field.value.chars().any(char::is_control) {
            return Err("A request field contains a control character".into());
        }
    }
    Ok(())
}

pub(crate) fn normalize_tags(tags: &[String]) -> Result<Vec<String>, String> {
    let mut unique = Vec::new();
    for tag in tags {
        let Some(tag) = normalize_tag(tag) else {
            return Err("http_requests.json has an invalid tag".into());
        };
        if unique
            .iter()
            .any(|existing: &String| tag_key(existing) == tag_key(&tag))
        {
            continue;
        }
        if unique.len() >= MAX_TAGS {
            return Err(format!("A request has more than {MAX_TAGS} tags"));
        }
        unique.push(tag);
    }
    Ok(unique)
}

pub(crate) fn normalize_tag(raw: &str) -> Option<String> {
    let tag = raw.trim();
    if tag.is_empty()
        || tag.chars().count() > MAX_TAG_CHARS
        || tag
            .chars()
            .any(|symbol| symbol.is_control() || symbol == ',')
    {
        return None;
    }
    Some(tag.to_owned())
}

pub(crate) fn tag_key(tag: &str) -> String {
    tag.to_lowercase()
}

pub(crate) fn limit_chars(text: &mut String, max: usize) {
    if text.chars().count() > max {
        *text = text.chars().take(max).collect();
    }
}

pub(crate) fn content_type_kind(value: &str) -> Option<BodyKind> {
    let value = value.to_ascii_lowercase();
    if value.contains("application/json") || value.contains("+json") {
        Some(BodyKind::Json)
    } else if value.contains("application/x-www-form-urlencoded") {
        Some(BodyKind::Form)
    } else if value.contains("multipart/form-data") {
        Some(BodyKind::Multipart)
    } else if value.contains("text/plain") {
        Some(BodyKind::Text)
    } else {
        None
    }
}

/// Moves a typed query string out of the URL and into query rows.
pub(crate) fn absorb_url_query(request: &mut Request) {
    let Some((base, query)) = split_query(&request.url) else {
        return;
    };
    request.url = base;
    if !query.is_empty() {
        request.query = query;
    }
}

fn split_query(url: &str) -> Option<(String, Vec<Field>)> {
    let (before_fragment, fragment) = match url.split_once('#') {
        Some((base, fragment)) => (base, Some(fragment)),
        None => (url, None),
    };
    let (base, query) = before_fragment.split_once('?')?;
    let mut url = base.to_owned();
    if let Some(fragment) = fragment {
        url.push('#');
        url.push_str(fragment);
    }
    Some((url, parse_query(query)))
}

pub(crate) fn parse_query(query: &str) -> Vec<Field> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((name, value)) => {
                Field::new(percent_decode(name, false), percent_decode(value, false))
            }
            None => Field::new(percent_decode(part, false), String::new()),
        })
        .collect()
}

pub(crate) fn percent_decode(input: &str, plus_is_space: bool) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push((high << 4) | low);
            index += 3;
            continue;
        }
        if plus_is_space && bytes[index] == b'+' {
            out.push(b' ');
        } else {
            out.push(bytes[index]);
        }
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

pub(crate) fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(char::from(byte));
            }
            _ => {
                const HEX: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(char::from(HEX[usize::from(byte >> 4)]));
                out.push(char::from(HEX[usize::from(byte & 0x0f)]));
            }
        }
    }
    out
}

/// Keeps the `Content-Type` row aligned with the body kind when the user changes the kind.
pub(crate) fn apply_body_kind(request: &mut Request, kind: BodyKind) {
    let previous = request.body_kind.default_content_type();
    request.body_kind = kind;
    let next = kind.default_content_type();
    let index = request
        .headers
        .iter()
        .position(|field| field.name.eq_ignore_ascii_case("content-type"));
    match (index, previous, next) {
        (Some(index), Some(previous), Some(next)) if request.headers[index].value == previous => {
            next.clone_into(&mut request.headers[index].value);
        }
        (Some(index), Some(previous), None) if request.headers[index].value == previous => {
            request.headers.remove(index);
        }
        (None, _, Some(next)) => request.headers.push(Field::new("Content-Type", next)),
        _ => {}
    }
}

pub(crate) fn note_content_type(request: &mut Request) {
    let Some(value) = request
        .headers
        .iter()
        .find(|field| field.enabled && field.name.eq_ignore_ascii_case("content-type"))
        .map(|field| field.value.clone())
    else {
        return;
    };
    if let Some(kind) = content_type_kind(&value) {
        request.body_kind = kind;
    }
}
