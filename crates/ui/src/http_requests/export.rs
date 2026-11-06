//! Insert one saved call into an `OpenAPI` 3 or Swagger 2 JSON document.
#![allow(clippy::redundant_pub_crate)]

use super::model::{BodyKind, Field, Request};
use serde_json::{Map, Value};

/// Returns the document with this request added or replaced at the same path and method.
///
/// An empty document becomes a minimal `OpenAPI` 3.0.3 file. Existing responses and sibling
/// methods stay. Pretty-printing rewrites the whole JSON object and keeps key order.
///
/// # Errors
/// Returns an error when the text is not a JSON object or is not `OpenAPI` 3 / Swagger 2.
pub(crate) fn upsert(text: &str, request: &Request) -> Result<String, String> {
    let trimmed = text.trim().trim_start_matches('\u{feff}');
    let mut root = if trimmed.is_empty() {
        template()
    } else {
        serde_json::from_str::<Value>(trimmed).map_err(|error| error.to_string())?
    };
    let object = root
        .as_object_mut()
        .ok_or_else(|| "OpenAPI document must be a JSON object".to_string())?;
    if object.contains_key("swagger") {
        upsert_swagger(object, request)?;
    } else {
        if !object.contains_key("openapi") {
            object.insert("openapi".into(), Value::String("3.0.3".into()));
        }
        let version = object
            .get("openapi")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        if !version.starts_with('3') {
            return Err("Only OpenAPI 3.x and Swagger 2.0 JSON documents are supported".into());
        }
        ensure_info(object);
        upsert_v3(object, request)?;
    }
    let mut bytes = serde_json::to_vec_pretty(&root).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

fn template() -> Value {
    serde_json::json!({
        "openapi": "3.0.3",
        "info": {"title": "API", "version": "0.1.0"},
        "paths": {}
    })
}

fn ensure_info(object: &mut Map<String, Value>) {
    if object.get("info").and_then(Value::as_object).is_none() {
        object.insert(
            "info".into(),
            serde_json::json!({"title": "API", "version": "0.1.0"}),
        );
    }
}

fn upsert_v3(object: &mut Map<String, Value>, request: &Request) -> Result<(), String> {
    let (origin, path) = origin_and_path(&request.url);
    let servers_missing = object
        .get("servers")
        .is_none_or(|value| value.as_array().is_some_and(Vec::is_empty));
    if let Some(origin) = origin
        && servers_missing
    {
        object.insert(
            "servers".into(),
            Value::Array(vec![serde_json::json!({"url": origin})]),
        );
    }
    let operation = operation_mut(object, &path, &request.method)?;
    write_summary(operation, request);
    operation.insert(
        "parameters".into(),
        Value::Array(parameters(request, &path, true)),
    );
    operation.remove("requestBody");
    if let Some(body) = openapi_body(request) {
        operation.insert("requestBody".into(), body);
    }
    if operation
        .get("parameters")
        .and_then(Value::as_array)
        .is_some_and(Vec::is_empty)
    {
        operation.remove("parameters");
    }
    Ok(())
}

fn upsert_swagger(object: &mut Map<String, Value>, request: &Request) -> Result<(), String> {
    let version = object.get("swagger").and_then(Value::as_str).unwrap_or("");
    if version != "2.0" {
        return Err("Only OpenAPI 3.x and Swagger 2.0 JSON documents are supported".into());
    }
    ensure_info(object);
    let (origin, mut path) = origin_and_path(&request.url);
    if object.get("host").and_then(Value::as_str).is_none()
        && let Some(origin) = &origin
        && let Some((scheme, host)) = origin.split_once("://")
    {
        object.insert(
            "schemes".into(),
            Value::Array(vec![Value::String(scheme.to_owned())]),
        );
        object.insert("host".into(), Value::String(host.to_owned()));
        object
            .entry("basePath")
            .or_insert_with(|| Value::String("/".into()));
    }
    if let Some(base) = object
        .get("basePath")
        .and_then(Value::as_str)
        .map(str::to_owned)
        && base.len() > 1
        && let Some(rest) = path.strip_prefix(&base)
    {
        path = if rest.is_empty() {
            "/".into()
        } else if rest.starts_with('/') {
            rest.to_owned()
        } else {
            format!("/{rest}")
        };
    }
    let operation = operation_mut(object, &path, &request.method)?;
    write_summary(operation, request);
    let mut parameters = parameters(request, &path, false);
    match request.body_kind {
        BodyKind::Json | BodyKind::Text => {
            operation.insert(
                "consumes".into(),
                Value::Array(vec![Value::String(content_type(request))]),
            );
            parameters.push(serde_json::json!({
                "name": "body",
                "in": "body",
                "required": !request.body.is_empty(),
                "schema": {"type": "string"},
                "example": request.body
            }));
        }
        BodyKind::Form | BodyKind::Multipart => {
            operation.insert(
                "consumes".into(),
                Value::Array(vec![Value::String(content_type(request))]),
            );
            for field in enabled(&request.form) {
                parameters.push(serde_json::json!({
                    "name": field.name,
                    "in": "formData",
                    "type": "string",
                    "example": field.value
                }));
            }
        }
        BodyKind::None => {
            operation.remove("consumes");
        }
    }
    if parameters.is_empty() {
        operation.remove("parameters");
    } else {
        operation.insert("parameters".into(), Value::Array(parameters));
    }
    Ok(())
}

fn operation_mut<'a>(
    object: &'a mut Map<String, Value>,
    path: &str,
    method: &str,
) -> Result<&'a mut Map<String, Value>, String> {
    let method = method_key(method)?;
    let paths = object
        .entry("paths")
        .or_insert_with(|| Value::Object(Map::new()));
    let paths = paths
        .as_object_mut()
        .ok_or_else(|| "OpenAPI paths must be an object".to_string())?;
    let item = paths
        .entry(path.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    let item = item
        .as_object_mut()
        .ok_or_else(|| "OpenAPI path item must be an object".to_string())?;
    let entry = item
        .entry(method)
        .or_insert_with(|| Value::Object(Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(Map::new());
    }
    entry
        .as_object_mut()
        .ok_or_else(|| "OpenAPI operation must be an object".to_string())
}

fn write_summary(operation: &mut Map<String, Value>, request: &Request) {
    if request.tags.is_empty() {
        operation.remove("tags");
    } else {
        operation.insert(
            "tags".into(),
            Value::Array(request.tags.iter().cloned().map(Value::String).collect()),
        );
    }
    operation.insert("summary".into(), Value::String(request.name.clone()));
    if operation.get("responses").is_none() {
        operation.insert(
            "responses".into(),
            serde_json::json!({"200": {"description": "OK"}}),
        );
    }
}

fn parameters(request: &Request, path: &str, openapi3: bool) -> Vec<Value> {
    let mut parameters = Vec::new();
    for name in template_names(path) {
        parameters.push(if openapi3 {
            serde_json::json!({"name": name, "in": "path", "required": true, "schema": {"type": "string"}})
        } else {
            serde_json::json!({"name": name, "in": "path", "required": true, "type": "string"})
        });
    }
    let names = template_names(path);
    for field in enabled(&request.query) {
        if names.iter().any(|name| name == &field.name) {
            continue;
        }
        parameters.push(if openapi3 {
            serde_json::json!({"name": field.name, "in": "query", "schema": {"type": "string"}, "example": field.value})
        } else {
            serde_json::json!({"name": field.name, "in": "query", "type": "string", "example": field.value})
        });
    }
    for field in enabled(&request.headers) {
        if field.name.eq_ignore_ascii_case("content-type")
            || field.name.eq_ignore_ascii_case("content-length")
            || field.name.eq_ignore_ascii_case("host")
        {
            continue;
        }
        parameters.push(if openapi3 {
            serde_json::json!({"name": field.name, "in": "header", "schema": {"type": "string"}, "example": field.value})
        } else {
            serde_json::json!({"name": field.name, "in": "header", "type": "string", "example": field.value})
        });
    }
    parameters
}

fn openapi_body(request: &Request) -> Option<Value> {
    let content = content_type(request);
    let media = match request.body_kind {
        BodyKind::None => return None,
        BodyKind::Json => {
            let example = serde_json::from_str::<Value>(&request.body)
                .unwrap_or_else(|_| Value::String(request.body.clone()));
            let schema_type = if request.body.trim_start().starts_with('[') {
                "array"
            } else {
                "object"
            };
            serde_json::json!({"schema": {"type": schema_type}, "example": example})
        }
        BodyKind::Text => {
            serde_json::json!({"schema": {"type": "string"}, "example": request.body})
        }
        BodyKind::Form | BodyKind::Multipart => {
            let mut properties = Map::new();
            let mut example = Map::new();
            for field in enabled(&request.form) {
                properties.insert(field.name.clone(), serde_json::json!({"type": "string"}));
                example.insert(field.name.clone(), Value::String(field.value.clone()));
            }
            serde_json::json!({
                "schema": {"type": "object", "properties": properties},
                "example": example
            })
        }
    };
    Some(serde_json::json!({"content": {content: media}}))
}

fn content_type(request: &Request) -> String {
    request
        .headers
        .iter()
        .find(|field| field.enabled && field.name.eq_ignore_ascii_case("content-type"))
        .map(|field| field.value.clone())
        .filter(|value| !value.is_empty())
        .or_else(|| request.body_kind.default_content_type().map(str::to_owned))
        .unwrap_or_else(|| match request.body_kind {
            BodyKind::Multipart => "multipart/form-data".into(),
            _ => "application/octet-stream".into(),
        })
}

fn method_key(method: &str) -> Result<String, String> {
    if method.is_empty() || !method.chars().all(|symbol| symbol.is_ascii_alphabetic()) {
        return Err("HTTP method must be letters".into());
    }
    Ok(method.to_ascii_lowercase())
}

pub(crate) fn origin_and_path(url: &str) -> (Option<String>, String) {
    let url = url.trim();
    let (url, _) = url.split_once('#').unwrap_or((url, ""));
    let (url, _) = url.split_once('?').unwrap_or((url, ""));
    let Some((scheme, after)) = url.split_once("://") else {
        return (None, normalize_path(url));
    };
    if scheme != "http" && scheme != "https" {
        return (None, normalize_path(url));
    }
    let (host, path) = after
        .find('/')
        .map_or((after, "/"), |index| (&after[..index], &after[index..]));
    if host.is_empty() {
        return (None, normalize_path(path));
    }
    (Some(format!("{scheme}://{host}")), normalize_path(path))
}

fn normalize_path(path: &str) -> String {
    if path.is_empty() {
        "/".into()
    } else if path.starts_with('/') {
        path.to_owned()
    } else {
        format!("/{path}")
    }
}

fn template_names(path: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        rest = &rest[start + 1..];
        let Some(end) = rest.find('}') else {
            break;
        };
        let name = &rest[..end];
        if !name.is_empty() && !name.contains('{') && !names.iter().any(|existing| existing == name)
        {
            names.push(name.to_owned());
        }
        rest = &rest[end + 1..];
    }
    names
}

fn enabled(fields: &[Field]) -> impl Iterator<Item = &Field> {
    fields
        .iter()
        .filter(|field| field.enabled && !field.name.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Request {
        let mut request = Request::blank("h1".into(), 1);
        request.name = "Create user".into();
        request.method = "POST".into();
        request.url = "https://example.com/users/{id}".into();
        request.query = vec![Field::new("verbose", "1")];
        request.headers = vec![
            Field::new("Accept", "application/json"),
            Field::new("Content-Type", "application/json"),
        ];
        request.body_kind = BodyKind::Json;
        request.body = "{\"name\":\"Ada\"}".into();
        request.tags = vec!["users".into()];
        request
    }

    #[test]
    fn creates_openapi3_and_updates_the_same_operation() -> Result<(), String> {
        let request = sample();
        let text = upsert("", &request)?;
        let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
        assert_eq!(value["openapi"], "3.0.3");
        assert_eq!(value["servers"][0]["url"], "https://example.com");
        let operation = &value["paths"]["/users/{id}"]["post"];
        assert_eq!(operation["summary"], "Create user");
        assert_eq!(operation["tags"][0], "users");
        assert_eq!(operation["parameters"][0]["in"], "path");
        assert_eq!(operation["parameters"][0]["name"], "id");
        assert_eq!(
            operation["requestBody"]["content"]["application/json"]["example"]["name"],
            "Ada"
        );
        let mut again = request;
        again.name = "Update user".into();
        let text = upsert(&text, &again)?;
        let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
        assert_eq!(
            value["paths"]["/users/{id}"]["post"]["summary"],
            "Update user"
        );
        assert!(value["paths"]["/users/{id}"].get("get").is_none());
        Ok(())
    }

    #[test]
    fn keeps_a_sibling_method_and_writes_swagger() -> Result<(), String> {
        let existing = r#"{
            "openapi": "3.0.3",
            "info": {"title": "API", "version": "1"},
            "paths": {"/users/{id}": {"get": {"summary": "Read", "responses": {"200": {"description": "ok"}}}}}
        }"#;
        let text = upsert(existing, &sample())?;
        let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
        assert_eq!(value["paths"]["/users/{id}"]["get"]["summary"], "Read");
        assert_eq!(
            value["paths"]["/users/{id}"]["post"]["summary"],
            "Create user"
        );
        assert_eq!(
            value["paths"]["/users/{id}"]["get"]["responses"]["200"]["description"],
            "ok"
        );

        let swagger = r#"{"swagger":"2.0","info":{"title":"API","version":"1"},"paths":{}}"#;
        let text = upsert(swagger, &sample())?;
        let value: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
        assert_eq!(value["host"], "example.com");
        assert_eq!(value["schemes"][0], "https");
        assert_eq!(
            value["paths"]["/users/{id}"]["post"]["parameters"][0]["in"],
            "path"
        );
        let body = value["paths"]["/users/{id}"]["post"]["parameters"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["in"] == "body"))
            .ok_or("missing swagger body")?;
        assert_eq!(body["example"], "{\"name\":\"Ada\"}");
        Ok(())
    }

    #[test]
    fn rejects_a_non_object() {
        assert!(upsert("[1]", &sample()).is_err());
        assert!(upsert("{", &sample()).is_err());
    }
}
