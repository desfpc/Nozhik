//! `OpenAPI` 3.x and Swagger 2.0 view model, examples and request assembly.
use serde_json::{Map, Value};

const MAX_DEPTH: usize = 8;
const MAX_PROPERTIES: usize = 40;

#[derive(Clone, Debug)]
pub struct Spec {
    pub title: String,
    pub version: String,
    pub openapi: String,
    pub description: String,
    pub info_extra: String,
    pub external_docs: String,
    pub servers: Vec<Server>,
    pub schemes: Vec<Scheme>,
    pub operations: Vec<Operation>,
    pub groups: Vec<Group>,
    pub schemas: Vec<SchemaModel>,
    pub global_security: Security,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Server {
    pub url: String,
    pub description: String,
    pub variables: Vec<ServerVar>,
}

#[derive(Clone, Debug)]
pub struct ServerVar {
    pub name: String,
    pub default_value: String,
    pub choices: Vec<String>,
    pub description: String,
}

#[derive(Clone, Debug)]
pub struct Scheme {
    pub name: String,
    pub title: String,
    pub description: String,
    pub kind: SchemeKind,
}

#[derive(Clone, Debug)]
pub enum SchemeKind {
    ApiKey { name: String, location: KeyIn },
    Basic,
    Bearer { format: String },
    Http { scheme: String },
    OAuth2 { flows: String },
    OpenId { url: String },
    Other { type_name: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyIn {
    Query,
    Header,
    Cookie,
}

#[derive(Clone, Debug)]
pub struct Group {
    pub name: String,
    pub description: String,
    pub operations: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Operation {
    pub key: String,
    pub method: String,
    pub path: String,
    pub summary: String,
    pub description: String,
    pub identifier: String,
    pub deprecated: bool,
    pub tags: Vec<String>,
    pub parameters: Vec<Parameter>,
    pub body: Option<Body>,
    pub responses: Vec<ResponseDoc>,
    pub security: Security,
    pub callbacks: Vec<String>,
    pub external_docs: String,
    pub servers: Vec<Server>,
}

#[derive(Clone, Debug)]
pub enum Security {
    Inherit,
    None,
    Requirements(Vec<Vec<String>>),
}

#[derive(Clone, Debug)]
pub struct Parameter {
    pub id: String,
    pub name: String,
    pub location: ParamIn,
    pub required: bool,
    pub description: String,
    pub type_name: String,
    pub example: String,
    pub choices: Vec<String>,
    pub shape: Shape,
    pub encoding: Encoding,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamIn {
    Path,
    Query,
    Header,
    Cookie,
}

impl ParamIn {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Path => "path",
            Self::Query => "query",
            Self::Header => "header",
            Self::Cookie => "cookie",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shape {
    Text,
    Boolean,
    Json,
}

#[derive(Clone, Copy, Debug)]
pub struct Encoding {
    pub style: ParamStyle,
    pub explode: bool,
    pub reserved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamStyle {
    Simple,
    Form,
    Space,
    Pipe,
    DeepObject,
}

#[derive(Clone, Debug)]
pub struct Body {
    pub required: bool,
    pub description: String,
    pub contents: Vec<Media>,
}

#[derive(Clone, Debug)]
pub struct Media {
    pub content_type: String,
    pub example: String,
    pub schema_text: String,
}

#[derive(Clone, Debug)]
pub struct ResponseDoc {
    pub code: String,
    pub description: String,
    pub links: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct SchemaModel {
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Debug)]
pub struct Prepared {
    pub method: String,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Option<Vec<u8>>,
    pub curl: String,
}

#[derive(Clone, Debug, Default)]
pub struct AuthInput {
    pub authorized: bool,
    pub token: String,
    pub username: String,
    pub password: String,
}

pub fn parse_document(value: &Value) -> Result<Spec, String> {
    let root = value
        .as_object()
        .ok_or_else(|| "OpenAPI document must be a JSON object".to_string())?;
    let mut warnings = Vec::new();
    let openapi = version_label(root);
    let info = root.get("info").and_then(Value::as_object);
    let title = info
        .and_then(|o| o.get("title"))
        .and_then(Value::as_str)
        .unwrap_or("API")
        .to_string();
    let version = info
        .and_then(|o| o.get("version"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let description = info
        .and_then(|o| o.get("description"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let info_extra = info_line(info);
    let external_docs = docs_url(root.get("externalDocs"));
    let servers = servers_of(root, &openapi);
    let schemes = schemes_of(root);
    let global_security = security_of(root.get("security"));
    let models = schemas_of(root);
    let tag_meta = tag_meta(root);
    let operations = operations_of(value, &mut warnings);
    let groups = groups_of(&operations, &tag_meta);
    Ok(Spec {
        title,
        version,
        openapi,
        description,
        info_extra,
        external_docs,
        servers,
        schemes,
        operations,
        groups,
        schemas: models,
        global_security,
        warnings,
    })
}

fn version_label(root: &Map<String, Value>) -> String {
    if let Some(value) = root.get("openapi").and_then(Value::as_str) {
        return value.to_string();
    }
    root.get("swagger")
        .and_then(Value::as_str)
        .unwrap_or("3.0.0")
        .to_string()
}

fn info_line(info: Option<&Map<String, Value>>) -> String {
    let Some(info) = info else {
        return String::new();
    };
    let mut parts = Vec::new();
    if let Some(contact) = info.get("contact").and_then(Value::as_object) {
        for key in ["name", "email", "url"] {
            if let Some(value) = contact.get(key).and_then(Value::as_str)
                && !value.is_empty()
            {
                parts.push(value.to_string());
            }
        }
    }
    if let Some(license) = info.get("license").and_then(Value::as_object) {
        let name = license.get("name").and_then(Value::as_str).unwrap_or("");
        let url = license.get("url").and_then(Value::as_str).unwrap_or("");
        if !name.is_empty() && !url.is_empty() {
            parts.push(format!("{name} ({url})"));
        } else if !name.is_empty() {
            parts.push(name.to_string());
        } else if !url.is_empty() {
            parts.push(url.to_string());
        }
    }
    parts.join(" · ")
}

fn docs_url(node: Option<&Value>) -> String {
    node.and_then(|v| v.get("url"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn servers_of(root: &Map<String, Value>, openapi: &str) -> Vec<Server> {
    if openapi.starts_with('2') {
        return vec![swagger_server(root)];
    }
    let Some(list) = root.get("servers").and_then(Value::as_array) else {
        return vec![Server {
            url: "/".into(),
            description: String::new(),
            variables: Vec::new(),
        }];
    };
    let servers: Vec<_> = list.iter().filter_map(server_one).collect();
    if servers.is_empty() {
        vec![Server {
            url: "/".into(),
            description: String::new(),
            variables: Vec::new(),
        }]
    } else {
        servers
    }
}

fn swagger_server(root: &Map<String, Value>) -> Server {
    let scheme = root
        .get("schemes")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
        .unwrap_or("https");
    let host = root
        .get("host")
        .and_then(Value::as_str)
        .unwrap_or("localhost");
    let base = root.get("basePath").and_then(Value::as_str).unwrap_or("");
    Server {
        url: format!("{scheme}://{host}{base}"),
        description: "Swagger host".into(),
        variables: Vec::new(),
    }
}

fn server_one(value: &Value) -> Option<Server> {
    let url = value.get("url")?.as_str()?.to_string();
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut variables = Vec::new();
    if let Some(map) = value.get("variables").and_then(Value::as_object) {
        for (name, spec) in map {
            variables.push(ServerVar {
                name: name.clone(),
                default_value: spec.get("default").map(scalar_text).unwrap_or_default(),
                choices: enum_strings(spec.get("enum")),
                description: spec
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string(),
            });
        }
    }
    Some(Server {
        url,
        description,
        variables,
    })
}

fn schemes_of(root: &Map<String, Value>) -> Vec<Scheme> {
    let map = root
        .get("components")
        .and_then(|v| v.get("securitySchemes"))
        .and_then(Value::as_object)
        .or_else(|| root.get("securityDefinitions").and_then(Value::as_object));
    let Some(map) = map else {
        return Vec::new();
    };
    map.iter()
        .map(|(name, value)| scheme_one(name, value))
        .collect()
}

fn scheme_one(name: &str, value: &Value) -> Scheme {
    let description = value
        .get("description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let type_name = value
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_ascii_lowercase();
    let kind = match type_name.as_str() {
        "apikey" => SchemeKind::ApiKey {
            name: value
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or(name)
                .to_string(),
            location: match value.get("in").and_then(Value::as_str).unwrap_or("header") {
                "query" => KeyIn::Query,
                "cookie" => KeyIn::Cookie,
                _ => KeyIn::Header,
            },
        },
        "basic" => SchemeKind::Basic,
        "http" => {
            let scheme = value
                .get("scheme")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_ascii_lowercase();
            match scheme.as_str() {
                "basic" => SchemeKind::Basic,
                "bearer" => SchemeKind::Bearer {
                    format: value
                        .get("bearerFormat")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                },
                _ => SchemeKind::Http { scheme },
            }
        }
        "oauth2" => SchemeKind::OAuth2 {
            flows: oauth_flows(value),
        },
        "openidconnect" => SchemeKind::OpenId {
            url: value
                .get("openIdConnectUrl")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        },
        _ => SchemeKind::Other { type_name },
    };
    let kind_label = match &kind {
        SchemeKind::ApiKey { .. } => "apiKey",
        SchemeKind::Basic => "http basic",
        SchemeKind::Bearer { .. } => "http bearer",
        SchemeKind::Http { scheme } => scheme.as_str(),
        SchemeKind::OAuth2 { .. } => "oauth2",
        SchemeKind::OpenId { .. } => "openIdConnect",
        SchemeKind::Other { type_name } => type_name.as_str(),
    };
    Scheme {
        name: name.to_string(),
        title: format!("{name} ({kind_label})"),
        description,
        kind,
    }
}

fn oauth_flows(value: &Value) -> String {
    let Some(flows) = value.get("flows").and_then(Value::as_object) else {
        return "Paste an access token".into();
    };
    let mut lines = Vec::new();
    for (name, flow) in flows {
        let url = flow
            .get("authorizationUrl")
            .or_else(|| flow.get("tokenUrl"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if url.is_empty() {
            lines.push(name.clone());
        } else {
            lines.push(format!("{name}: {url}"));
        }
    }
    if lines.is_empty() {
        "Paste an access token".into()
    } else {
        format!("Paste an access token. {}", lines.join("; "))
    }
}

fn security_of(value: Option<&Value>) -> Security {
    let Some(list) = value.and_then(Value::as_array) else {
        return Security::Inherit;
    };
    if list.is_empty() {
        return Security::None;
    }
    let requirements = list
        .iter()
        .filter_map(Value::as_object)
        .map(|item| item.keys().cloned().collect())
        .collect();
    Security::Requirements(requirements)
}

fn schemas_of(root: &Map<String, Value>) -> Vec<SchemaModel> {
    let map = root
        .get("components")
        .and_then(|v| v.get("schemas"))
        .and_then(Value::as_object)
        .or_else(|| root.get("definitions").and_then(Value::as_object));
    let Some(map) = map else {
        return Vec::new();
    };
    map.iter()
        .map(|(name, value)| SchemaModel {
            name: name.clone(),
            value: value.clone(),
        })
        .collect()
}

fn tag_meta(root: &Map<String, Value>) -> Vec<(String, String)> {
    root.get("tags")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = item.get("name")?.as_str()?.to_string();
                    let description = item
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string();
                    Some((name, description))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn operations_of(root: &Value, warnings: &mut Vec<String>) -> Vec<Operation> {
    let mut operations = Vec::new();
    if let Some(paths) = root.get("paths").and_then(Value::as_object) {
        for (path, item) in paths {
            push_path(root, path, item, false, warnings, &mut operations);
        }
    }
    if let Some(hooks) = root.get("webhooks").and_then(Value::as_object) {
        for (name, item) in hooks {
            let path = if name.starts_with('/') {
                name.clone()
            } else {
                format!("/{name}")
            };
            push_path(root, &path, item, true, warnings, &mut operations);
        }
    }
    operations
}

fn push_path(
    root: &Value,
    path: &str,
    item: &Value,
    webhook: bool,
    warnings: &mut Vec<String>,
    operations: &mut Vec<Operation>,
) {
    let mut stack = Vec::new();
    let resolved = match materialize(root, item, &mut stack) {
        Ok(value) => value,
        Err(error) => {
            warnings.push(format!("{path}: {error}"));
            return;
        }
    };
    let Some(object) = resolved.as_object() else {
        warnings.push(format!("{path}: path item is not an object"));
        return;
    };
    let shared = parameter_list(root, object.get("parameters"), warnings);
    let path_servers: Vec<Server> = object
        .get("servers")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(server_one).collect())
        .unwrap_or_default();
    for (method, operation) in object {
        if !is_method(method) {
            continue;
        }
        match operation_one(
            root,
            path,
            method,
            operation,
            &shared,
            &path_servers,
            webhook,
            warnings,
        ) {
            Ok(operation) => operations.push(operation),
            Err(error) => warnings.push(format!("{method} {path}: {error}")),
        }
    }
}

fn is_method(name: &str) -> bool {
    matches!(
        name,
        "get" | "put" | "post" | "delete" | "options" | "head" | "patch" | "trace"
    )
}

#[allow(
    clippy::too_many_arguments,
    reason = "One operation needs the document, path, method, shared parameters, servers and webhook flag"
)]
fn operation_one(
    root: &Value,
    path: &str,
    method: &str,
    node: &Value,
    shared: &[Parameter],
    path_servers: &[Server],
    webhook: bool,
    warnings: &mut Vec<String>,
) -> Result<Operation, String> {
    let mut stack = Vec::new();
    let resolved = materialize(root, node, &mut stack)?;
    let object = resolved
        .as_object()
        .ok_or_else(|| "operation is not an object".to_string())?;
    let method_name = method.to_ascii_uppercase();
    let mut tags = object
        .get("tags")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if webhook && !tags.iter().any(|tag| tag == "webhooks") {
        tags.insert(0, "webhooks".into());
    }
    let mut parameters = shared.to_vec();
    let own = parameter_list(root, object.get("parameters"), warnings);
    for parameter in own {
        if let Some(slot) = parameters
            .iter_mut()
            .find(|item| item.name == parameter.name && item.location == parameter.location)
        {
            *slot = parameter;
        } else {
            parameters.push(parameter);
        }
    }
    let body = request_body(root, object, warnings);
    let servers = object
        .get("servers")
        .and_then(Value::as_array)
        .map(|items| items.iter().filter_map(server_one).collect())
        .filter(|items: &Vec<_>| !items.is_empty())
        .unwrap_or_else(|| path_servers.to_vec());
    let callbacks = object
        .get("callbacks")
        .and_then(Value::as_object)
        .map(|map| map.keys().cloned().collect())
        .unwrap_or_default();
    Ok(Operation {
        key: format!("{method_name} {path}"),
        method: method_name,
        path: path.to_string(),
        summary: object
            .get("summary")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        description: object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        identifier: object
            .get("operationId")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        deprecated: object
            .get("deprecated")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        tags,
        parameters,
        body,
        responses: responses_of(object.get("responses")),
        security: security_of(object.get("security")),
        callbacks,
        external_docs: docs_url(object.get("externalDocs")),
        servers,
    })
}

fn parameter_list(
    root: &Value,
    node: Option<&Value>,
    warnings: &mut Vec<String>,
) -> Vec<Parameter> {
    let Some(list) = node.and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut parameters = Vec::new();
    for item in list {
        match parameter_one(root, item) {
            Ok(Some(parameter)) => parameters.push(parameter),
            Ok(None) => {}
            Err(error) => warnings.push(error),
        }
    }
    parameters
}

fn parameter_one(root: &Value, node: &Value) -> Result<Option<Parameter>, String> {
    let mut stack = Vec::new();
    let resolved = materialize(root, node, &mut stack)?;
    let object = resolved
        .as_object()
        .ok_or_else(|| "parameter is not an object".to_string())?;
    let location = match object.get("in").and_then(Value::as_str).unwrap_or("") {
        "path" => ParamIn::Path,
        "query" => ParamIn::Query,
        "header" => ParamIn::Header,
        "cookie" => ParamIn::Cookie,
        "body" | "formData" => return Ok(None),
        other => {
            return Err(format!("unsupported parameter location {other}"));
        }
    };
    let name = object
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if name.is_empty() {
        return Err("parameter without a name".into());
    }
    let schema = parameter_schema(object);
    let required = object
        .get("required")
        .and_then(Value::as_bool)
        .unwrap_or(location == ParamIn::Path);
    Ok(Some(Parameter {
        id: format!("{}:{name}", location.label()),
        name,
        location,
        required,
        description: object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        type_name: type_label(&schema),
        example: parameter_example(object, &schema),
        choices: enum_strings(schema.get("enum")),
        shape: shape_of(&schema),
        encoding: encoding_of(location, object),
    }))
}

fn parameter_schema(object: &Map<String, Value>) -> Value {
    if let Some(schema) = object.get("schema") {
        return schema.clone();
    }
    let mut schema = Map::new();
    if let Some(kind) = object.get("type") {
        schema.insert("type".into(), kind.clone());
    }
    if let Some(format) = object.get("format") {
        schema.insert("format".into(), format.clone());
    }
    if let Some(values) = object.get("enum") {
        schema.insert("enum".into(), values.clone());
    }
    if let Some(default) = object.get("default") {
        schema.insert("default".into(), default.clone());
    }
    if let Some(items) = object.get("items") {
        schema.insert("items".into(), items.clone());
    }
    Value::Object(schema)
}

fn parameter_example(object: &Map<String, Value>, schema: &Value) -> String {
    if let Some(value) = object.get("example") {
        return scalar_text(value);
    }
    if let Some(value) = schema.get("example") {
        return scalar_text(value);
    }
    if let Some(value) = schema.get("default").or_else(|| object.get("default")) {
        return scalar_text(value);
    }
    String::new()
}

fn encoding_of(location: ParamIn, object: &Map<String, Value>) -> Encoding {
    let (style, explode) = match location {
        ParamIn::Query | ParamIn::Cookie => (ParamStyle::Form, true),
        ParamIn::Path | ParamIn::Header => (ParamStyle::Simple, false),
    };
    let style = object
        .get("style")
        .and_then(Value::as_str)
        .map_or(style, ParamStyle::parse);
    Encoding {
        style,
        explode: object
            .get("explode")
            .and_then(Value::as_bool)
            .unwrap_or(explode),
        reserved: object
            .get("allowReserved")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    }
}

impl ParamStyle {
    fn parse(value: &str) -> Self {
        match value {
            "form" => Self::Form,
            "spaceDelimited" => Self::Space,
            "pipeDelimited" => Self::Pipe,
            "deepObject" => Self::DeepObject,
            _ => Self::Simple,
        }
    }
}

fn request_body(
    root: &Value,
    object: &Map<String, Value>,
    warnings: &mut Vec<String>,
) -> Option<Body> {
    if let Some(node) = object.get("requestBody") {
        return match body_from_request(root, node, warnings) {
            Ok(body) => body,
            Err(error) => {
                warnings.push(error);
                None
            }
        };
    }
    swagger_body(root, object, warnings)
}

fn body_from_request(
    root: &Value,
    node: &Value,
    warnings: &mut Vec<String>,
) -> Result<Option<Body>, String> {
    let mut stack = Vec::new();
    let resolved = materialize(root, node, &mut stack)?;
    let object = resolved
        .as_object()
        .ok_or_else(|| "requestBody is not an object".to_string())?;
    let contents = media_list(root, object.get("content"), warnings);
    if contents.is_empty() {
        return Ok(None);
    }
    Ok(Some(Body {
        required: object
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        description: object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        contents,
    }))
}

fn swagger_body(
    root: &Value,
    object: &Map<String, Value>,
    warnings: &mut Vec<String>,
) -> Option<Body> {
    let parameters = object.get("parameters")?.as_array()?;
    let mut form = Vec::new();
    let mut json_body = None;
    for item in parameters {
        let mut stack = Vec::new();
        let Ok(resolved) = materialize(root, item, &mut stack) else {
            continue;
        };
        let Some(parameter) = resolved.as_object() else {
            continue;
        };
        match parameter.get("in").and_then(Value::as_str) {
            Some("body") => {
                let schema = parameter
                    .get("schema")
                    .cloned()
                    .unwrap_or(Value::Object(Map::new()));
                let content_type = consumes(object).unwrap_or_else(|| "application/json".into());
                json_body = Some(media_for(root, &content_type, &schema, None, warnings));
            }
            Some("formData") => form.push(parameter.clone()),
            _ => {}
        }
    }
    if let Some(media) = json_body {
        return Some(Body {
            required: true,
            description: String::new(),
            contents: vec![media],
        });
    }
    if form.is_empty() {
        return None;
    }
    let mut properties = Map::new();
    let mut required = Vec::new();
    for parameter in &form {
        let name = parameter
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("field");
        properties.insert(name.to_string(), parameter_schema(parameter));
        if parameter
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            required.push(Value::String(name.to_string()));
        }
    }
    let mut schema = Map::new();
    schema.insert("type".into(), Value::String("object".into()));
    schema.insert("properties".into(), Value::Object(properties));
    if !required.is_empty() {
        schema.insert("required".into(), Value::Array(required));
    }
    let content_type = if form.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("file")
            || consumes(object).is_some_and(|value| value.contains("multipart"))
    }) {
        "multipart/form-data"
    } else {
        "application/x-www-form-urlencoded"
    };
    Some(Body {
        required: !form.is_empty(),
        description: String::new(),
        contents: vec![media_for(
            root,
            content_type,
            &Value::Object(schema),
            None,
            warnings,
        )],
    })
}

fn consumes(object: &Map<String, Value>) -> Option<String> {
    object
        .get("consumes")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn media_list(root: &Value, node: Option<&Value>, warnings: &mut Vec<String>) -> Vec<Media> {
    let Some(map) = node.and_then(Value::as_object) else {
        return Vec::new();
    };
    map.iter()
        .map(|(content_type, media)| {
            let schema = media
                .get("schema")
                .cloned()
                .unwrap_or(Value::Object(Map::new()));
            media_for(root, content_type, &schema, Some(media), warnings)
        })
        .collect()
}

fn media_for(
    root: &Value,
    content_type: &str,
    schema: &Value,
    media: Option<&Value>,
    warnings: &mut Vec<String>,
) -> Media {
    let explicit = media.and_then(media_example);
    let example_value = explicit.unwrap_or_else(|| {
        let mut stack = Vec::new();
        match example_value(root, schema, &mut stack, 0) {
            Ok(value) => value,
            Err(error) => {
                warnings.push(error);
                Value::Object(Map::new())
            }
        }
    });
    let schema_owned = {
        let mut stack = Vec::new();
        materialize(root, schema, &mut stack).unwrap_or_else(|_| schema.clone())
    };
    Media {
        content_type: content_type.to_string(),
        example: render_example(content_type, &example_value),
        schema_text: serde_json::to_string_pretty(&schema_owned)
            .unwrap_or_else(|_| schema_owned.to_string()),
    }
}

fn media_example(media: &Value) -> Option<Value> {
    if let Some(example) = media.get("example") {
        return Some(example.clone());
    }
    let examples = media.get("examples")?.as_object()?;
    let first = examples.values().next()?;
    first.get("value").cloned().or_else(|| Some(first.clone()))
}

fn render_example(content_type: &str, value: &Value) -> String {
    if content_type.contains("json") || content_type.ends_with("+json") {
        return serde_json::to_string_pretty(value).unwrap_or_else(|_| value.to_string());
    }
    if content_type.contains("x-www-form-urlencoded") {
        return form_pairs(value);
    }
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

fn responses_of(node: Option<&Value>) -> Vec<ResponseDoc> {
    let Some(map) = node.and_then(Value::as_object) else {
        return Vec::new();
    };
    map.iter()
        .map(|(code, value)| ResponseDoc {
            code: code.clone(),
            description: value
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            links: value
                .get("links")
                .and_then(Value::as_object)
                .map(|links| links.keys().cloned().collect())
                .unwrap_or_default(),
        })
        .collect()
}

fn groups_of(operations: &[Operation], tag_meta: &[(String, String)]) -> Vec<Group> {
    let mut names = Vec::new();
    for (name, _) in tag_meta {
        if !names.iter().any(|existing: &String| existing == name) {
            names.push(name.clone());
        }
    }
    for operation in operations {
        let tags = operation_tags(operation);
        for tag in tags {
            if !names.iter().any(|existing| existing == &tag) {
                names.push(tag);
            }
        }
    }
    let mut groups = Vec::new();
    for name in names {
        let operations = operations
            .iter()
            .enumerate()
            .filter(|(_, operation)| operation_tags(operation).iter().any(|tag| tag == &name))
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if operations.is_empty() {
            continue;
        }
        let description = tag_meta
            .iter()
            .find(|(candidate, _)| candidate == &name)
            .map(|(_, description)| description.clone())
            .unwrap_or_default();
        groups.push(Group {
            name,
            description,
            operations,
        });
    }
    groups
}

fn operation_tags(operation: &Operation) -> Vec<String> {
    if operation.tags.is_empty() {
        vec!["default".into()]
    } else {
        operation.tags.clone()
    }
}

fn materialize(root: &Value, node: &Value, stack: &mut Vec<String>) -> Result<Value, String> {
    if stack.len() > MAX_DEPTH {
        return Err("schema $ref is too deep".into());
    }
    let Some(reference) = node.get("$ref").and_then(Value::as_str) else {
        return Ok(node.clone());
    };
    if stack.iter().any(|item| item == reference) {
        return Err(format!("cyclic $ref {reference}"));
    }
    let next = pointer(root, reference).ok_or_else(|| format!("unresolved $ref {reference}"))?;
    stack.push(reference.to_string());
    let mut resolved = materialize(root, next, stack)?;
    stack.pop();
    if let (Value::Object(base), Some(extra)) = (&mut resolved, node.as_object()) {
        for (key, value) in extra {
            if key != "$ref" {
                base.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(resolved)
}

fn pointer<'a>(root: &'a Value, reference: &str) -> Option<&'a Value> {
    if reference == "#" {
        return Some(root);
    }
    let path = reference.strip_prefix("#/")?;
    let mut current = root;
    for raw in path.split('/') {
        let key = decode_pointer(raw);
        current = match current {
            Value::Object(map) => map.get(&key)?,
            Value::Array(items) => items.get(key.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(current)
}

fn decode_pointer(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars();
    while let Some(character) = chars.next() {
        if character == '~' {
            match chars.next() {
                Some('0') | None => out.push('~'),
                Some('1') => out.push('/'),
                Some(other) => {
                    out.push('~');
                    out.push(other);
                }
            }
        } else {
            out.push(character);
        }
    }
    out
}

fn example_value(
    root: &Value,
    schema: &Value,
    stack: &mut Vec<String>,
    depth: usize,
) -> Result<Value, String> {
    if depth > MAX_DEPTH {
        return Ok(Value::Object(Map::new()));
    }
    let schema = match materialize(root, schema, stack) {
        Ok(value) => value,
        Err(error) if error.contains("cyclic") || error.contains("too deep") => {
            return Ok(Value::Object(Map::new()));
        }
        Err(error) => return Err(error),
    };
    if let Some(example) = schema.get("example").filter(|value| !value.is_null()) {
        return Ok(example.clone());
    }
    if let Some(default) = schema.get("default").filter(|value| !value.is_null()) {
        return Ok(default.clone());
    }
    if let Some(constant) = schema.get("const") {
        return Ok(constant.clone());
    }
    if let Some(first) = schema
        .get("enum")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    {
        return Ok(first.clone());
    }
    if let Some(items) = schema.get("allOf").and_then(Value::as_array) {
        return merge_examples(root, items, stack, depth);
    }
    if let Some(first) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        .and_then(|items| items.first())
    {
        return example_value(root, first, stack, depth + 1);
    }
    match schema_type(&schema) {
        Some("string") => Ok(Value::String(string_example(&schema))),
        Some("integer") => Ok(serde_json::json!(0)),
        Some("number") => Ok(number_zero()),
        Some("boolean") => Ok(Value::Bool(true)),
        Some("null") => Ok(Value::Null),
        Some("array") => {
            let items = schema
                .get("items")
                .cloned()
                .unwrap_or(Value::Object(Map::new()));
            Ok(Value::Array(vec![example_value(
                root,
                &items,
                stack,
                depth + 1,
            )?]))
        }
        _ => object_example(root, &schema, stack, depth),
    }
}

fn merge_examples(
    root: &Value,
    items: &[Value],
    stack: &mut Vec<String>,
    depth: usize,
) -> Result<Value, String> {
    let mut merged = Map::new();
    let mut fallback = None;
    for item in items {
        match example_value(root, item, stack, depth + 1)? {
            Value::Object(part) => {
                for (key, value) in part {
                    merged.insert(key, value);
                }
            }
            other => fallback = Some(other),
        }
    }
    if merged.is_empty() {
        Ok(fallback.unwrap_or(Value::Object(Map::new())))
    } else {
        Ok(Value::Object(merged))
    }
}

fn object_example(
    root: &Value,
    schema: &Value,
    stack: &mut Vec<String>,
    depth: usize,
) -> Result<Value, String> {
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Ok(Value::Object(Map::new()));
    };
    let mut map = Map::new();
    for (name, property) in properties.iter().take(MAX_PROPERTIES) {
        map.insert(
            name.clone(),
            example_value(root, property, stack, depth + 1)?,
        );
    }
    Ok(Value::Object(map))
}

fn string_example(schema: &Value) -> String {
    match schema.get("format").and_then(Value::as_str).unwrap_or("") {
        "email" | "idn-email" => "user@example.com".into(),
        "date" => "2020-01-01".into(),
        "date-time" => "2020-01-01T00:00:00Z".into(),
        "uuid" => "00000000-0000-0000-0000-000000000000".into(),
        "uri" | "url" | "uri-reference" => "https://example.com".into(),
        "hostname" => "example.com".into(),
        "ipv4" => "127.0.0.1".into(),
        "ipv6" => "::1".into(),
        "byte" | "binary" => String::new(),
        _ => "string".into(),
    }
}

fn number_zero() -> Value {
    serde_json::Number::from_f64(0.0).map_or_else(|| serde_json::json!(0), Value::Number)
}

fn schema_type(schema: &Value) -> Option<&str> {
    match schema.get("type")? {
        Value::String(value) => Some(value.as_str()),
        Value::Array(items) => items
            .iter()
            .filter_map(Value::as_str)
            .find(|value| *value != "null"),
        _ => None,
    }
}

fn type_label(schema: &Value) -> String {
    if let Some(reference) = schema.get("$ref").and_then(Value::as_str) {
        return reference
            .rsplit('/')
            .next()
            .unwrap_or(reference)
            .to_string();
    }
    schema_type(schema)
        .unwrap_or_else(|| {
            if schema.get("properties").is_some() || schema.get("allOf").is_some() {
                "object"
            } else {
                "string"
            }
        })
        .to_string()
}

fn shape_of(schema: &Value) -> Shape {
    match schema_type(schema) {
        Some("boolean") => Shape::Boolean,
        Some("array" | "object") => Shape::Json,
        _ if schema.get("properties").is_some() => Shape::Json,
        _ => Shape::Text,
    }
}

fn enum_strings(node: Option<&Value>) -> Vec<String> {
    node.and_then(Value::as_array)
        .map(|items| items.iter().map(scalar_text).collect())
        .unwrap_or_default()
}

fn scalar_text(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(flag) => flag.to_string(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

pub fn server_label(server: &Server) -> String {
    if server.description.is_empty() {
        server.url.clone()
    } else {
        format!("{} - {}", server.url, server.description)
    }
}

pub fn resolve_server(server: &Server, values: &[String]) -> String {
    let mut url = server.url.clone();
    for (index, variable) in server.variables.iter().enumerate() {
        let value = values.get(index).map_or("", String::as_str);
        let chosen = if value.is_empty() {
            variable.default_value.as_str()
        } else {
            value
        };
        url = url.replace(&format!("{{{}}}", variable.name), chosen);
    }
    url
}

pub struct CallDraft<'a> {
    pub server_index: usize,
    pub server_values: &'a [String],
    pub params: &'a std::collections::HashMap<String, String>,
    pub content_index: usize,
    pub body_text: &'a str,
    pub auth: &'a std::collections::HashMap<String, AuthInput>,
}

pub fn prepare(
    spec: &Spec,
    operation: &Operation,
    draft: &CallDraft<'_>,
) -> Result<Prepared, String> {
    let server_index = draft.server_index;
    let server_values = draft.server_values;
    let params = draft.params;
    let content_index = draft.content_index;
    let body_text = draft.body_text;
    let auth = draft.auth;
    let servers = if operation.servers.is_empty() {
        spec.servers.as_slice()
    } else {
        operation.servers.as_slice()
    };
    let server = servers
        .get(server_index)
        .ok_or_else(|| "Selected server is missing".to_string())?;
    let base = resolve_server(server, server_values);
    let path = fill_path(&operation.path, &operation.parameters, params)?;
    let mut url = join_url(&base, &path);
    let mut query = Vec::new();
    let mut headers = Vec::new();
    let mut cookies = Vec::new();
    for parameter in &operation.parameters {
        if parameter.location == ParamIn::Path {
            continue;
        }
        let value = params.get(&parameter.id).map_or("", String::as_str);
        if value.is_empty() {
            if parameter.required {
                return Err(format!(
                    "Parameter {} ({}) is required",
                    parameter.name,
                    parameter.location.label()
                ));
            }
            continue;
        }
        match parameter.location {
            ParamIn::Query => push_query(&mut query, parameter, value),
            ParamIn::Header => headers.push((parameter.name.clone(), value.to_string())),
            ParamIn::Cookie => cookies.push(format!(
                "{}={}",
                encode(&parameter.name),
                encode_cookie(value, parameter.encoding.reserved)
            )),
            ParamIn::Path => {}
        }
    }
    if !cookies.is_empty() {
        headers.push(("Cookie".into(), cookies.join("; ")));
    }
    apply_auth(spec, operation, auth, &mut query, &mut headers);
    append_query(&mut url, &query);
    let (body, content_type) = encode_body(operation, content_index, body_text)?;
    if let Some(content_type) = content_type {
        upsert_header(&mut headers, "Content-Type", content_type);
    }
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return Err("Server URL must start with http:// or https://".into());
    }
    let method = operation.method.clone();
    let curl = curl_of(&method, &url, &headers, body.as_deref());
    Ok(Prepared {
        method,
        url,
        headers,
        body,
        curl,
    })
}

/// Builds the HTTP Requests record for the same call Try it out would send.
///
/// # Errors
/// Returns the same errors as [`prepare`]: a missing server, a required parameter, or a bad body.
pub fn transfer_request(
    spec: &Spec,
    operation: &Operation,
    draft: &CallDraft<'_>,
) -> Result<crate::http_requests::Incoming, String> {
    let prepared = prepare(spec, operation, draft)?;
    Ok(incoming_from(operation, &prepared))
}

fn incoming_from(operation: &Operation, prepared: &Prepared) -> crate::http_requests::Incoming {
    use crate::http_requests::{Incoming, MAX_TAGS, normalize_tag};
    let content_type = prepared
        .headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case("content-type"))
        .map(|(_, value)| value.as_str());
    let (body_kind, body, form) = split_transfer_body(content_type, prepared.body.as_deref());
    let tags = operation
        .tags
        .iter()
        .filter_map(|tag| normalize_tag(tag))
        .take(MAX_TAGS)
        .collect();
    Incoming {
        name: clean_transfer_name(&operation.summary, &operation.method, &operation.path),
        method: prepared.method.clone(),
        url: prepared.url.clone(),
        query: Vec::new(),
        headers: prepared.headers.clone(),
        body_kind,
        body,
        form,
        tags,
    }
}

fn clean_transfer_name(summary: &str, method: &str, path: &str) -> String {
    let raw = if summary.trim().is_empty() {
        format!("{method} {path}")
    } else {
        summary.to_owned()
    };
    let cleaned: String = raw
        .chars()
        .map(|symbol| if symbol.is_control() { ' ' } else { symbol })
        .collect();
    let trimmed = cleaned.trim();
    if trimmed.is_empty() {
        "Untitled request".into()
    } else {
        trimmed.to_owned()
    }
}

fn split_transfer_body(
    content_type: Option<&str>,
    bytes: Option<&[u8]>,
) -> (
    crate::http_requests::BodyKind,
    String,
    Vec<(String, String)>,
) {
    use crate::http_requests::{BodyKind, content_type_kind};
    let Some(bytes) = bytes.filter(|bytes| !bytes.is_empty()) else {
        return (BodyKind::None, String::new(), Vec::new());
    };
    let text = String::from_utf8_lossy(bytes).into_owned();
    match content_type.and_then(content_type_kind) {
        Some(BodyKind::Form) => (BodyKind::Form, String::new(), form_transfer_fields(&text)),
        Some(BodyKind::Multipart) => {
            let fields = content_type
                .and_then(boundary_value)
                .map(|boundary| multipart_transfer_fields(&text, boundary))
                .unwrap_or_default();
            if fields.is_empty() {
                (BodyKind::Text, text, Vec::new())
            } else {
                (BodyKind::Multipart, String::new(), fields)
            }
        }
        Some(BodyKind::Json) => (BodyKind::Json, text, Vec::new()),
        Some(BodyKind::Text | BodyKind::None) | None => (BodyKind::Text, text, Vec::new()),
    }
}

fn form_transfer_fields(body: &str) -> Vec<(String, String)> {
    use crate::http_requests::percent_decode;
    body.split('&')
        .filter(|part| !part.is_empty())
        .map(|part| match part.split_once('=') {
            Some((name, value)) => (percent_decode(name, true), percent_decode(value, true)),
            None => (percent_decode(part, true), String::new()),
        })
        .collect()
}

fn boundary_value(content_type: &str) -> Option<&str> {
    let lower = content_type.to_ascii_lowercase();
    let index = lower.find("boundary=")? + "boundary=".len();
    let rest = content_type.get(index..)?.trim();
    let rest = rest.trim_start_matches('"');
    let end = rest.find(['"', ';', ' ', '\r', '\n']).unwrap_or(rest.len());
    let value = rest.get(..end)?.trim();
    (!value.is_empty()).then_some(value)
}

fn multipart_transfer_fields(body: &str, boundary: &str) -> Vec<(String, String)> {
    let marker = format!("--{boundary}");
    let mut fields = Vec::new();
    for part in body.split(&marker) {
        let part = part.trim_start_matches(['\r', '\n']);
        if part.is_empty() || part.starts_with("--") {
            continue;
        }
        let Some((headers, content)) = part
            .split_once("\r\n\r\n")
            .or_else(|| part.split_once("\n\n"))
        else {
            continue;
        };
        let Some(name) = disposition_name(headers) else {
            continue;
        };
        let value = content.trim_end_matches("\r\n").trim_end_matches('\n');
        fields.push((name, value.to_owned()));
    }
    fields
}

fn disposition_name(headers: &str) -> Option<String> {
    for line in headers.split(['\n', '\r']) {
        let lower = line.to_ascii_lowercase();
        if !lower.contains("content-disposition") {
            continue;
        }
        let index = lower.find("name=")? + "name=".len();
        let rest = line.get(index..)?.trim().trim_matches('"');
        let end = rest.find(['"', ';']).unwrap_or(rest.len());
        let name = rest.get(..end)?.trim();
        if name.is_empty() {
            return None;
        }
        return Some(name.to_owned());
    }
    None
}

fn fill_path(
    template: &str,
    parameters: &[Parameter],
    params: &std::collections::HashMap<String, String>,
) -> Result<String, String> {
    let mut path = template.to_string();
    for parameter in parameters
        .iter()
        .filter(|item| item.location == ParamIn::Path)
    {
        let value = params.get(&parameter.id).map_or("", String::as_str);
        if value.is_empty() {
            return Err(format!("Path parameter {} is required", parameter.name));
        }
        let encoded = if parameter.encoding.reserved {
            value.to_string()
        } else {
            encode(value)
        };
        let token = format!("{{{}}}", parameter.name);
        if !path.contains(&token) {
            return Err(format!("Path has no {{{}}} placeholder", parameter.name));
        }
        path = path.replace(&token, &encoded);
    }
    Ok(path)
}

fn push_query(query: &mut Vec<(String, String)>, parameter: &Parameter, value: &str) {
    let parsed = if parameter.shape == Shape::Json {
        serde_json::from_str::<Value>(value).ok()
    } else {
        None
    };
    match (parameter.encoding.style, parsed) {
        (ParamStyle::DeepObject, Some(Value::Object(map))) => {
            for (key, item) in map {
                query.push((format!("{}[{key}]", parameter.name), scalar_text(&item)));
            }
        }
        (_, Some(Value::Array(items))) if parameter.encoding.explode => {
            for item in items {
                query.push((parameter.name.clone(), scalar_text(&item)));
            }
        }
        (_, Some(Value::Array(items))) => {
            let separator = match parameter.encoding.style {
                ParamStyle::Space => " ",
                ParamStyle::Pipe => "|",
                _ => ",",
            };
            let combined = items
                .iter()
                .map(scalar_text)
                .collect::<Vec<_>>()
                .join(separator);
            query.push((parameter.name.clone(), combined));
        }
        (_, Some(Value::Object(map)))
            if parameter.encoding.explode && parameter.encoding.style == ParamStyle::Form =>
        {
            for (key, item) in map {
                query.push((key, scalar_text(&item)));
            }
        }
        _ => query.push((parameter.name.clone(), value.to_string())),
    }
}

fn apply_auth(
    spec: &Spec,
    operation: &Operation,
    auth: &std::collections::HashMap<String, AuthInput>,
    query: &mut Vec<(String, String)>,
    headers: &mut Vec<(String, String)>,
) {
    let requirements = match &operation.security {
        Security::None => return,
        Security::Inherit => match &spec.global_security {
            Security::Requirements(items) => items,
            _ => return,
        },
        Security::Requirements(items) => items,
    };
    let Some(group) = requirements.iter().find(|group| {
        !group.is_empty()
            && group.iter().all(|name| {
                auth.get(name)
                    .is_some_and(|input| input.authorized && auth_ready(spec, name, input))
            })
    }) else {
        return;
    };
    for name in group {
        let Some(input) = auth.get(name) else {
            continue;
        };
        let Some(scheme) = spec.schemes.iter().find(|scheme| &scheme.name == name) else {
            continue;
        };
        apply_scheme(scheme, input, query, headers);
    }
}

fn auth_ready(spec: &Spec, name: &str, input: &AuthInput) -> bool {
    let Some(scheme) = spec.schemes.iter().find(|scheme| scheme.name == name) else {
        return false;
    };
    match scheme.kind {
        SchemeKind::Basic => !input.username.is_empty(),
        _ => !input.token.is_empty(),
    }
}

fn apply_scheme(
    scheme: &Scheme,
    input: &AuthInput,
    query: &mut Vec<(String, String)>,
    headers: &mut Vec<(String, String)>,
) {
    match &scheme.kind {
        SchemeKind::ApiKey { name, location } => match location {
            KeyIn::Header => upsert_header(headers, name, input.token.clone()),
            KeyIn::Query => query.push((name.clone(), input.token.clone())),
            KeyIn::Cookie => {
                let cookie = format!("{}={}", encode(name), encode(&input.token));
                match headers
                    .iter_mut()
                    .find(|(key, _)| key.eq_ignore_ascii_case("cookie"))
                {
                    Some((_, value)) => {
                        if !value.is_empty() {
                            value.push_str("; ");
                        }
                        value.push_str(&cookie);
                    }
                    None => headers.push(("Cookie".into(), cookie)),
                }
            }
        },
        SchemeKind::Basic => {
            use base64::Engine;
            let raw = format!("{}:{}", input.username, input.password);
            let encoded = base64::engine::general_purpose::STANDARD.encode(raw.as_bytes());
            upsert_header(headers, "Authorization", format!("Basic {encoded}"));
        }
        SchemeKind::Bearer { .. } | SchemeKind::OAuth2 { .. } | SchemeKind::OpenId { .. } => {
            let token = input.token.trim();
            let value = if token.to_ascii_lowercase().starts_with("bearer ") {
                token.to_string()
            } else {
                format!("Bearer {token}")
            };
            upsert_header(headers, "Authorization", value);
        }
        SchemeKind::Http { .. } | SchemeKind::Other { .. } => {
            upsert_header(headers, "Authorization", input.token.clone());
        }
    }
}

fn upsert_header(headers: &mut Vec<(String, String)>, name: &str, value: String) {
    if let Some((_, slot)) = headers
        .iter_mut()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
    {
        *slot = value;
    } else {
        headers.push((name.to_string(), value));
    }
}

fn encode_body(
    operation: &Operation,
    content_index: usize,
    body_text: &str,
) -> Result<(Option<Vec<u8>>, Option<String>), String> {
    let Some(body) = &operation.body else {
        return Ok((None, None));
    };
    if body_text.is_empty() {
        return Ok((None, None));
    }
    let media = body
        .contents
        .get(content_index)
        .or_else(|| body.contents.first())
        .ok_or_else(|| "Request body has no content type".to_string())?;
    if media.content_type.contains("x-www-form-urlencoded")
        && body_text.trim_start().starts_with('{')
    {
        let value: Value = serde_json::from_str(body_text)
            .map_err(|error| format!("Form body is not valid JSON: {error}"))?;
        let encoded = form_pairs(&value);
        return Ok((Some(encoded.into_bytes()), Some(media.content_type.clone())));
    }
    if media.content_type.contains("multipart/form-data") && body_text.trim_start().starts_with('{')
    {
        let value: Value = serde_json::from_str(body_text)
            .map_err(|error| format!("Multipart body is not valid JSON: {error}"))?;
        let (content_type, bytes) = multipart(&value)?;
        return Ok((Some(bytes), Some(content_type)));
    }
    Ok((
        Some(body_text.to_string().into_bytes()),
        Some(media.content_type.clone()),
    ))
}

fn form_pairs(value: &Value) -> String {
    let Some(map) = value.as_object() else {
        return scalar_text(value);
    };
    map.iter()
        .map(|(key, item)| format!("{}={}", encode(key), encode(&scalar_text(item))))
        .collect::<Vec<_>>()
        .join("&")
}

fn multipart(value: &Value) -> Result<(String, Vec<u8>), String> {
    let map = value
        .as_object()
        .ok_or_else(|| "Multipart body must be a JSON object".to_string())?;
    let boundary = "nozhikOpenApiBoundary";
    let mut body = Vec::new();
    for (key, item) in map {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            format!("Content-Disposition: form-data; name=\"{key}\"\r\n\r\n").as_bytes(),
        );
        body.extend_from_slice(scalar_text(item).as_bytes());
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    Ok((format!("multipart/form-data; boundary={boundary}"), body))
}

fn join_url(server: &str, path: &str) -> String {
    let server = server.trim_end_matches('/');
    if path.is_empty() {
        server.to_string()
    } else if let Some(path) = path.strip_prefix('/') {
        format!("{server}/{path}")
    } else {
        format!("{server}/{path}")
    }
}

fn append_query(url: &mut String, query: &[(String, String)]) {
    if query.is_empty() {
        return;
    }
    url.push(if url.contains('?') { '&' } else { '?' });
    for (index, (key, value)) in query.iter().enumerate() {
        if index > 0 {
            url.push('&');
        }
        url.push_str(&encode(key));
        url.push('=');
        url.push_str(&encode(value));
    }
}

fn encode(value: &str) -> String {
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

fn encode_cookie(value: &str, reserved: bool) -> String {
    if reserved {
        value.to_string()
    } else {
        encode(value)
    }
}

fn curl_of(method: &str, url: &str, headers: &[(String, String)], body: Option<&[u8]>) -> String {
    let mut parts = vec![format!("curl -X {} {}", method, shell_quote(url))];
    for (name, value) in headers {
        parts.push(format!("-H {}", shell_quote(&format!("{name}: {value}"))));
    }
    if let Some(body) = body {
        let text = String::from_utf8_lossy(body);
        parts.push(format!("-d {}", shell_quote(&text)));
    }
    parts.join(" \\\n  ")
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn schema_named<'a>(spec: &'a Spec, reference: &str) -> Option<&'a Value> {
    let name = reference.rsplit('/').next().unwrap_or(reference);
    spec.schemas
        .iter()
        .find(|schema| schema.name == name)
        .map(|schema| &schema.value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(text: &str) -> Result<Spec, String> {
        let value = serde_json::from_str(text).map_err(|error| error.to_string())?;
        parse_document(&value)
    }

    fn auth_map(name: &str, token: &str) -> std::collections::HashMap<String, AuthInput> {
        let mut map = std::collections::HashMap::new();
        map.insert(
            name.into(),
            AuthInput {
                authorized: true,
                token: token.into(),
                username: String::new(),
                password: String::new(),
            },
        );
        map
    }

    const SAMPLE: &str = r##"{
      "openapi": "3.0.3",
      "info": {"title": "nozhove.com API", "version": "1.0.0", "description": "HTTP API"},
      "servers": [
        {"url": "http://localhost:8181", "description": "Local Docker"},
        {"url": "https://nozhove.com", "description": "Production"}
      ],
      "components": {
        "securitySchemes": {
          "TokenAuth": {"type": "apiKey", "in": "header", "name": "Token", "description": "Access token"}
        },
        "schemas": {
          "ApiEnvelopeSuccess": {
            "type": "object",
            "properties": {"success": {"type": "boolean"}, "data": {"type": "object"}},
            "required": ["success", "data"]
          }
        }
      },
      "paths": {
        "/api/v1/profile/auth": {
          "post": {
            "summary": "Login with email and password",
            "requestBody": {"required": true, "content": {"application/json": {"schema": {
              "type": "object",
              "required": ["email", "password"],
              "properties": {
                "email": {"type": "string", "format": "email"},
                "password": {"type": "string"}
              }
            }}}},
            "responses": {"200": {"description": "JWT tokens"}, "401": {"description": "Wrong email or password"}}
          }
        },
        "/api/v1/profile/register": {
          "post": {
            "summary": "Register",
            "requestBody": {"required": true, "content": {"application/json": {"schema": {
              "type": "object",
              "properties": {
                "email": {"type": "string", "format": "email"},
                "password": {"type": "string"},
                "rePassword": {"type": "string"},
                "lang": {"type": "string", "enum": ["en", "ru"]}
              }
            }}}},
            "responses": {"200": {"description": "Created"}}
          }
        },
        "/api/v1/subscription/invoice/{id}": {
          "get": {
            "summary": "Pro invoice status for the owner",
            "security": [{"TokenAuth": []}],
            "parameters": [{"name": "id", "in": "path", "required": true, "schema": {"type": "integer"}}],
            "responses": {"200": {"description": "Invoice status payload"}, "401": {"description": "Unauthorized"}}
          }
        },
        "/api/v1/legal/{slug}": {
          "get": {
            "summary": "Public legal page",
            "parameters": [
              {"name": "slug", "in": "path", "required": true, "schema": {"type": "string", "enum": ["terms-of-use"]}},
              {"name": "lang", "in": "query", "schema": {"type": "string", "default": "en"}}
            ],
            "responses": {"200": {"description": "HTML", "content": {"application/json": {"schema": {"$ref": "#/components/schemas/ApiEnvelopeSuccess"}}}}}
          }
        }
      }
    }"##;

    #[test]
    fn sample_matches_the_editor_fixture() -> Result<(), String> {
        let spec = spec(SAMPLE)?;
        assert_eq!(spec.title, "nozhove.com API");
        assert_eq!(spec.openapi, "3.0.3");
        assert_eq!(spec.servers.len(), 2);
        let server = spec.servers.first().ok_or("server")?;
        assert_eq!(server_label(server), "http://localhost:8181 - Local Docker");
        assert_eq!(spec.groups.len(), 1);
        assert_eq!(spec.groups.first().ok_or("group")?.name, "default");
        assert_eq!(spec.operations.len(), 4);
        let login = spec.operations.first().ok_or("login")?;
        let body = login.body.as_ref().ok_or("body")?;
        assert!(body.required);
        let media = body.contents.first().ok_or("media")?;
        assert_eq!(
            media.example,
            "{\n  \"email\": \"user@example.com\",\n  \"password\": \"string\"\n}"
        );
        let register = spec.operations.get(1).ok_or("register")?;
        let example = &register
            .body
            .as_ref()
            .ok_or("body")?
            .contents
            .first()
            .ok_or("media")?
            .example;
        let email = example.find("email").ok_or("email")?;
        let password = example.find("password").ok_or("password")?;
        let repeat = example.find("rePassword").ok_or("repeat")?;
        let lang = example.find("lang").ok_or("lang")?;
        assert!(email < password && password < repeat && repeat < lang);
        let invoice = spec
            .operations
            .iter()
            .find(|item| item.path.ends_with("{id}"))
            .ok_or("invoice")?;
        let parameter = invoice.parameters.first().ok_or("parameter")?;
        assert!(parameter.required);
        assert_eq!(parameter.type_name, "integer");
        assert_eq!(parameter.example, "");
        assert!(matches!(invoice.security, Security::Requirements(_)));
        assert_eq!(
            spec.schemas.first().ok_or("schema")?.name,
            "ApiEnvelopeSuccess"
        );
        assert_eq!(
            spec.schemes.first().ok_or("scheme")?.title,
            "TokenAuth (apiKey)"
        );
        Ok(())
    }

    #[test]
    fn prepares_path_query_and_token_header() -> Result<(), String> {
        let spec = spec(SAMPLE)?;
        let operation = spec
            .operations
            .iter()
            .find(|item| item.method == "GET" && item.path.contains("{id}"))
            .ok_or("get")?;
        let mut params = std::collections::HashMap::new();
        params.insert("path:id".into(), "15".into());
        let call = prepare(
            &spec,
            operation,
            &CallDraft {
                server_index: 0,
                server_values: &[],
                params: &params,
                content_index: 0,
                body_text: "",
                auth: &auth_map("TokenAuth", "secret"),
            },
        )?;
        assert_eq!(call.method, "GET");
        assert_eq!(
            call.url,
            "http://localhost:8181/api/v1/subscription/invoice/15"
        );
        assert!(
            call.headers
                .iter()
                .any(|(name, value)| name == "Token" && value == "secret")
        );
        assert!(call.curl.contains("curl -X GET"));
        Ok(())
    }

    #[test]
    fn transfers_post_json_into_an_http_request() -> Result<(), String> {
        let value = serde_json::json!({
            "openapi": "3.0.3",
            "info": {"title": "Shop", "version": "1.0.0"},
            "servers": [{"url": "https://example.com"}],
            "paths": {
                "/login": {
                    "post": {
                        "summary": "Login",
                        "tags": ["auth"],
                        "parameters": [{
                            "name": "next",
                            "in": "query",
                            "schema": {"type": "string"}
                        }],
                        "requestBody": {
                            "content": {
                                "application/json": {"example": {"n": 1}}
                            }
                        },
                        "responses": {"200": {"description": "ok"}}
                    }
                }
            }
        });
        let spec = parse_document(&value)?;
        let operation = spec.operations.first().ok_or("operation")?;
        let mut params = std::collections::HashMap::new();
        params.insert("query:next".into(), "/home".into());
        let incoming = transfer_request(
            &spec,
            operation,
            &CallDraft {
                server_index: 0,
                server_values: &[],
                params: &params,
                content_index: 0,
                body_text: "{\"n\":1}",
                auth: &std::collections::HashMap::new(),
            },
        )?;
        assert_eq!(incoming.name, "Login");
        assert_eq!(incoming.method, "POST");
        assert!(incoming.url.starts_with("https://example.com/login?"));
        assert!(incoming.url.contains("next=%2Fhome"));
        assert_eq!(incoming.body_kind, crate::http_requests::BodyKind::Json);
        assert_eq!(incoming.body, "{\"n\":1}");
        assert_eq!(incoming.tags, vec!["auth".to_owned()]);
        Ok(())
    }

    #[test]
    fn encodes_reserved_characters_and_skips_optional_query() -> Result<(), String> {
        let spec = spec(SAMPLE)?;
        let operation = spec
            .operations
            .iter()
            .find(|item| item.path.contains("{slug}"))
            .ok_or("legal")?;
        let mut params = std::collections::HashMap::new();
        params.insert("path:slug".into(), "a b/c".into());
        let call = prepare(
            &spec,
            operation,
            &CallDraft {
                server_index: 1,
                server_values: &[],
                params: &params,
                content_index: 0,
                body_text: "",
                auth: &std::collections::HashMap::new(),
            },
        )?;
        assert_eq!(call.url, "https://nozhove.com/api/v1/legal/a%20b%2Fc");
        Ok(())
    }

    #[test]
    fn empty_operation_security_overrides_global_auth() -> Result<(), String> {
        let spec = spec(
            r#"{
            "openapi":"3.0.0",
            "info":{"title":"T","version":"1"},
            "servers":[{"url":"https://example.com"}],
            "components":{"securitySchemes":{"TokenAuth":{"type":"apiKey","in":"header","name":"Token"}}},
            "security":[{"TokenAuth":[]}],
            "paths":{"/open":{"get":{"security":[],"responses":{"200":{"description":"ok"}}}}}
        }"#,
        )?;
        let call = prepare(
            &spec,
            &spec.operations[0],
            &CallDraft {
                server_index: 0,
                server_values: &[],
                params: &std::collections::HashMap::new(),
                content_index: 0,
                body_text: "",
                auth: &auth_map("TokenAuth", "secret"),
            },
        )?;
        assert_eq!(call.headers, Vec::<(String, String)>::new());
        Ok(())
    }

    #[test]
    fn bearer_basic_and_server_variables() -> Result<(), String> {
        let spec = spec(
            r#"{
            "openapi":"3.1.0",
            "info":{"title":"T","version":"1"},
            "servers":[{"url":"https://{host}/v1","variables":{"host":{"default":"api.test","enum":["api.test"]}}}],
            "components":{"securitySchemes":{
              "User":{"type":"http","scheme":"basic"},
              "Jwt":{"type":"http","scheme":"bearer","bearerFormat":"JWT"}
            }},
            "paths":{
              "/a":{"get":{"security":[{"User":[]}],"responses":{"200":{"description":"ok"}}}},
              "/b":{"get":{"security":[{"Jwt":[]}],"responses":{"200":{"description":"ok"}}}}
            }
        }"#,
        )?;
        let server = spec.servers.first().ok_or("server")?;
        assert_eq!(
            resolve_server(server, &[String::new()]),
            "https://api.test/v1"
        );
        let mut basic = std::collections::HashMap::new();
        basic.insert(
            "User".into(),
            AuthInput {
                authorized: true,
                token: String::new(),
                username: "u".into(),
                password: "p".into(),
            },
        );
        let values = ["localhost".to_string()];
        let call = prepare(
            &spec,
            &spec.operations[0],
            &CallDraft {
                server_index: 0,
                server_values: &values,
                params: &std::collections::HashMap::new(),
                content_index: 0,
                body_text: "",
                auth: &basic,
            },
        )?;
        assert_eq!(call.url, "https://localhost/v1/a");
        assert!(call.headers.iter().any(|(_, value)| value == "Basic dTpw"));
        let bearer = prepare(
            &spec,
            &spec.operations[1],
            &CallDraft {
                server_index: 0,
                server_values: &[],
                params: &std::collections::HashMap::new(),
                content_index: 0,
                body_text: "",
                auth: &auth_map("Jwt", "abc"),
            },
        )?;
        assert!(
            bearer
                .headers
                .iter()
                .any(|(_, value)| value == "Bearer abc")
        );
        Ok(())
    }

    #[test]
    fn swagger2_body_and_form_are_requests() -> Result<(), String> {
        let spec = spec(
            r#"{
            "swagger":"2.0",
            "info":{"title":"Old","version":"1"},
            "host":"example.com",
            "basePath":"/v1",
            "schemes":["https"],
            "securityDefinitions":{"ApiKey":{"type":"apiKey","name":"X-Key","in":"header"}},
            "paths":{
              "/items":{"post":{
                "parameters":[{"name":"body","in":"body","required":true,"schema":{"type":"object","properties":{"name":{"type":"string"}}}}],
                "responses":{"201":{"description":"Created"}}
              }},
              "/form":{"post":{
                "consumes":["application/x-www-form-urlencoded"],
                "parameters":[{"name":"q","in":"formData","type":"string","required":true}],
                "responses":{"200":{"description":"ok"}}
              }}
            }
        }"#,
        )?;
        assert_eq!(
            spec.servers.first().ok_or("server")?.url,
            "https://example.com/v1"
        );
        let params = std::collections::HashMap::new();
        let example = spec
            .operations
            .first()
            .ok_or("json operation")?
            .body
            .as_ref()
            .ok_or("body")?
            .contents
            .first()
            .ok_or("media")?
            .example
            .clone();
        let call = prepare(
            &spec,
            &spec.operations[0],
            &CallDraft {
                server_index: 0,
                server_values: &[],
                params: &params,
                content_index: 0,
                body_text: &example,
                auth: &std::collections::HashMap::new(),
            },
        )?;
        assert_eq!(call.url, "https://example.com/v1/items");
        assert!(
            call.headers
                .iter()
                .any(|(_, value)| value == "application/json")
        );
        let form = spec.operations.get(1).ok_or("form operation")?;
        let text = form
            .body
            .as_ref()
            .ok_or("form")?
            .contents
            .first()
            .ok_or("media")?
            .example
            .clone();
        let sent = prepare(
            &spec,
            form,
            &CallDraft {
                server_index: 0,
                server_values: &[],
                params: &params,
                content_index: 0,
                body_text: &text,
                auth: &auth_map("ApiKey", "k"),
            },
        )?;
        assert!(
            sent.headers
                .iter()
                .any(|(_, value)| value.contains("urlencoded"))
        );
        let bytes = sent.body.ok_or("bytes")?;
        let body = String::from_utf8(bytes).map_err(|error| error.to_string())?;
        assert_eq!(body, "q=string");
        Ok(())
    }

    #[test]
    fn all_of_and_ref_example_and_relative_server_error() -> Result<(), String> {
        let spec = spec(
            r##"{
            "openapi":"3.0.0",
            "info":{"title":"T","version":"1"},
            "paths":{"/x":{"post":{
              "requestBody":{"content":{"application/json":{"schema":{
                "allOf":[
                  {"$ref":"#/components/schemas/A"},
                  {"type":"object","properties":{"extra":{"type":"integer"}}}
                ]
              }}}},
              "responses":{"200":{"description":"ok"}}
            }}},
            "components":{"schemas":{"A":{"type":"object","properties":{"name":{"type":"string"}}}}}
        }"##,
        )?;
        let example = &spec
            .operations
            .first()
            .ok_or("operation")?
            .body
            .as_ref()
            .ok_or("body")?
            .contents
            .first()
            .ok_or("media")?
            .example;
        assert!(example.contains("\"name\""));
        assert!(example.contains("\"extra\""));
        let Err(error) = prepare(
            &spec,
            &spec.operations[0],
            &CallDraft {
                server_index: 0,
                server_values: &[],
                params: &std::collections::HashMap::new(),
                content_index: 0,
                body_text: example,
                auth: &std::collections::HashMap::new(),
            },
        ) else {
            return Err("relative server was accepted".into());
        };
        assert!(error.contains("http://"));
        Ok(())
    }

    #[test]
    fn missing_path_parameter_is_an_error() -> Result<(), String> {
        let spec = spec(SAMPLE)?;
        let operation = spec
            .operations
            .iter()
            .find(|item| item.path.contains("{id}"))
            .ok_or("id")?;
        let Err(error) = prepare(
            &spec,
            operation,
            &CallDraft {
                server_index: 0,
                server_values: &[],
                params: &std::collections::HashMap::new(),
                content_index: 0,
                body_text: "",
                auth: &std::collections::HashMap::new(),
            },
        ) else {
            return Err("missing path parameter was accepted".into());
        };
        assert!(error.contains("id"));
        Ok(())
    }
}
