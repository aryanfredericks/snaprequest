//! OpenAPI 3.0 and 3.1 documents (JSON or YAML).
//!
//! Every operation becomes a request, in a folder per (first) tag. The first server becomes the
//! `base_url` variable; with several servers each one is an environment. Parameters and bodies
//! get example values (from `example`/`examples`/`default`/`enum`, or generated from the
//! schema), and security schemes become auth backed by secret variables.

use std::collections::HashMap;

use serde_json::{Map, Value, json};
use snapreq_collection::vars::valid_name;
use snapreq_collection::{
    AuthSpec, BodySpec, FolderFile, KeyValues, OAuth2Spec, PartSpec, RequestFile, Variable,
    Variables,
};
use snapreq_core::model::{ApiKeyLocation, KeyValue, OAuth2Grant};

use crate::error::{Error, Result};
use crate::model::{Imported, Node};
use crate::util::{is_json_type, literal, media_type, var_name};

const FORMAT: &str = "OpenAPI";
const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];
/// How deep example generation follows schemas (and `$ref` chains).
const MAX_DEPTH: usize = 8;

pub(crate) fn import(text: &str) -> Result<Imported> {
    let doc = crate::parse_document(text).map_err(|e| Error::invalid(FORMAT, e))?;
    let version = doc["openapi"].as_str().unwrap_or_default();
    if !version.starts_with("3.") {
        return Err(Error::Unsupported(format!(
            "OpenAPI version `{version}` is not supported (3.0 and 3.1 are)"
        )));
    }
    let mut spec = Spec {
        doc: &doc,
        imp: Imported::new(doc["info"]["title"].as_str().unwrap_or("Imported API")),
        auths: HashMap::new(),
    };
    spec.imp.reserve(text);
    spec.imp.collection.description = doc["info"]["description"]
        .as_str()
        .map(|d| d.trim().to_owned())
        .filter(|d| !d.is_empty());
    spec.servers();
    spec.imp.collection.auth = spec.auth_for(&doc["security"]);
    let global_security = doc["security"].clone();

    // Folders by tag: declared tags first, then in order of appearance.
    let mut folders: Vec<(String, Option<String>, Vec<Node>)> = doc["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| {
            Some((
                t["name"].as_str()?.to_owned(),
                t["description"].as_str().map(str::to_owned),
                Vec::new(),
            ))
        })
        .collect();
    let mut root = Vec::new();
    let mut count = 0;
    for (path, item) in doc["paths"].as_object().into_iter().flatten() {
        let item = spec.resolve(item, 0);
        let shared = &item["parameters"];
        for method in METHODS {
            let Some(op) = item.get(method) else { continue };
            let mut r = spec.request(method, path, op, shared);
            if !op["security"].is_null() && op["security"] != global_security {
                r.auth = Some(spec.auth_for(&op["security"]).unwrap_or(AuthSpec::None));
            }
            count += 1;
            match op["tags"].get(0).and_then(Value::as_str) {
                Some(tag) => {
                    let i = match folders.iter().position(|(n, _, _)| n == tag) {
                        Some(i) => i,
                        None => {
                            folders.push((tag.to_owned(), None, Vec::new()));
                            folders.len() - 1
                        }
                    };
                    folders[i].2.push(Node::Request(r));
                }
                None => root.push(Node::Request(r)),
            }
        }
    }
    if count == 0 {
        spec.imp.warn("the document has no operations");
    }
    let mut imp = spec.imp;
    for (name, description, items) in folders {
        if !items.is_empty() {
            imp.items.push(Node::Folder {
                name,
                file: FolderFile {
                    description,
                    ..FolderFile::default()
                },
                items,
            });
        }
    }
    imp.items.extend(root);
    Ok(imp)
}

struct Spec<'a> {
    doc: &'a Value,
    imp: Imported,
    /// Auth per security scheme name (built once, so warnings are not repeated).
    auths: HashMap<String, Option<AuthSpec>>,
}

impl<'a> Spec<'a> {
    /// Follows local `$ref`s; external ones resolve to `null` with a warning.
    fn resolve(&mut self, v: &'a Value, depth: usize) -> &'a Value {
        let mut v = v;
        for _ in depth..=MAX_DEPTH * 4 {
            let Some(r) = v.get("$ref").and_then(Value::as_str) else {
                return v;
            };
            match r.strip_prefix('#').and_then(|p| self.doc.pointer(p)) {
                Some(target) => v = target,
                None => {
                    self.imp.warn(format!(
                        "the reference `{r}` cannot be resolved; it was skipped"
                    ));
                    return &Value::Null;
                }
            }
        }
        &Value::Null
    }

    fn servers(&mut self) {
        let servers: Vec<(String, String)> = self.doc["servers"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(i, s)| {
                let mut url = s["url"].as_str()?.to_owned();
                for (name, var) in s["variables"].as_object().into_iter().flatten() {
                    let default = var["default"].as_str().unwrap_or_default();
                    url = url.replace(&format!("{{{name}}}"), default);
                }
                let label = s["description"]
                    .as_str()
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("server {}", i + 1));
                Some((label, url.trim_end_matches('/').to_owned()))
            })
            .collect();
        let base = match servers.first() {
            Some((_, url)) => url.clone(),
            None => {
                self.imp
                    .warn("the document lists no servers; set `base_url` to the API's address");
                "http://localhost".to_owned()
            }
        };
        if base.starts_with('/') {
            self.imp.warn(format!(
                "the server URL `{base}` is relative; `base_url` assumes http://localhost"
            ));
        }
        let absolute = |u: &str| {
            if u.starts_with('/') {
                format!("http://localhost{u}")
            } else {
                u.to_owned()
            }
        };
        self.imp.collection.vars.set("base_url", absolute(&base));
        if servers.len() > 1 {
            for (label, url) in &servers {
                let mut vars = Variables::default();
                vars.set("base_url", absolute(url));
                self.imp.add_environment(label, vars);
            }
        }
    }

    /// The auth for a `security` requirement list: `None` when absent, `none` when empty.
    fn auth_for(&mut self, security: &Value) -> Option<AuthSpec> {
        let list = security.as_array()?;
        let Some(first) = list.first().and_then(Value::as_object) else {
            return Some(AuthSpec::None);
        };
        let (name, scopes) = first.iter().next()?;
        if list.len() > 1 || first.len() > 1 {
            self.imp
                .warn("only the first security requirement of an operation is used".to_owned());
        }
        if let Some(found) = self.auths.get(name) {
            return found.clone();
        }
        let scheme = self.resolve(&self.doc["components"]["securitySchemes"][name.as_str()], 0);
        let scopes: Vec<String> = scopes
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|s| s.as_str().map(str::to_owned))
            .collect();
        let auth = self.scheme(name, scheme, &scopes);
        self.auths.insert(name.clone(), auth.clone());
        auth
    }

    fn scheme(&mut self, name: &str, s: &Value, scopes: &[String]) -> Option<AuthSpec> {
        let imp = &mut self.imp;
        match (
            s["type"].as_str(),
            s["scheme"].as_str().map(str::to_ascii_lowercase),
        ) {
            (Some("http"), Some(scheme)) if scheme == "basic" || scheme == "digest" => {
                plain(imp, "username");
                imp.declare_secret("password");
                let (username, password) = ("{{username}}".to_owned(), "{{password}}".to_owned());
                Some(if scheme == "basic" {
                    AuthSpec::Basic { username, password }
                } else {
                    AuthSpec::Digest { username, password }
                })
            }
            (Some("http"), Some(scheme)) if scheme == "bearer" => {
                imp.declare_secret("token");
                Some(AuthSpec::Bearer {
                    token: "{{token}}".into(),
                })
            }
            (Some("apiKey"), _) => {
                let key_name = s["name"].as_str().unwrap_or("X-API-Key").to_owned();
                let location = match s["in"].as_str() {
                    Some("query") => ApiKeyLocation::Query,
                    Some("header") => ApiKeyLocation::Header,
                    _ => {
                        imp.warn(format!(
                            "the API key `{name}` is sent in a cookie; add a `Cookie` header yourself"
                        ));
                        return None;
                    }
                };
                imp.declare_secret("api_key");
                Some(AuthSpec::ApiKey {
                    name: key_name,
                    value: "{{api_key}}".into(),
                    location,
                })
            }
            (Some("oauth2"), _) => {
                let flows = &s["flows"];
                let (grant, flow) = [
                    ("clientCredentials", OAuth2Grant::ClientCredentials),
                    ("authorizationCode", OAuth2Grant::AuthorizationCode),
                    ("password", OAuth2Grant::Password),
                ]
                .into_iter()
                .find_map(|(key, grant)| flows.get(key).map(|f| (grant, f)))
                .or_else(|| {
                    imp.warn(format!(
                        "the OAuth 2.0 scheme `{name}` has no client credentials, authorization code or password flow"
                    ));
                    None
                })?;
                let scope = if scopes.is_empty() {
                    flow["scopes"]
                        .as_object()
                        .map(|m| m.keys().cloned().collect::<Vec<_>>().join(" "))
                        .unwrap_or_default()
                } else {
                    scopes.join(" ")
                };
                plain(imp, "client_id");
                imp.declare_secret("client_secret");
                let mut spec = OAuth2Spec {
                    grant,
                    token_url: flow["tokenUrl"].as_str().unwrap_or_default().to_owned(),
                    auth_url: flow["authorizationUrl"].as_str().map(str::to_owned),
                    client_id: "{{client_id}}".into(),
                    client_secret: "{{client_secret}}".into(),
                    scope,
                    username: String::new(),
                    password: String::new(),
                    redirect_uri: None,
                    client_auth: None,
                    params: KeyValues::default(),
                };
                if grant == OAuth2Grant::Password {
                    plain(imp, "username");
                    imp.declare_secret("password");
                    spec.username = "{{username}}".into();
                    spec.password = "{{password}}".into();
                }
                Some(AuthSpec::OAuth2(spec))
            }
            _ => {
                imp.warn(format!(
                    "the security scheme `{name}` ({}) is not supported",
                    s["type"].as_str().unwrap_or("unknown")
                ));
                None
            }
        }
    }

    fn request(
        &mut self,
        method: &str,
        path: &str,
        op: &'a Value,
        shared: &'a Value,
    ) -> RequestFile {
        let mut url = String::from("{{base_url}}");
        // `{id}` → `{{id}}`
        let mut rest = path;
        while let Some(start) = rest.find('{') {
            let Some(len) = rest[start..].find('}') else {
                break;
            };
            url.push_str(&rest[..start]);
            url.push_str(&format!(
                "{{{{{}}}}}",
                var_name(&rest[start + 1..start + len])
            ));
            rest = &rest[start + len + 1..];
        }
        url.push_str(rest);

        let mut r = RequestFile::new(method, url);
        let summary = op["summary"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let fallback = format!("{} {path}", method.to_ascii_uppercase());
        r.name = Some(
            summary
                .or_else(|| op["operationId"].as_str())
                .unwrap_or(&fallback)
                .to_owned(),
        );
        r.description = op["description"]
            .as_str()
            .map(|d| d.trim().to_owned())
            .filter(|d| !d.is_empty());

        // Operation parameters override path-level ones with the same name and location.
        let mut params: Vec<&'a Value> = Vec::new();
        for p in op["parameters"].as_array().into_iter().flatten() {
            params.push(self.resolve(p, 0));
        }
        for p in shared.as_array().into_iter().flatten() {
            let p = self.resolve(p, 0);
            if !params
                .iter()
                .any(|q| q["name"] == p["name"] && q["in"] == p["in"])
            {
                params.push(p);
            }
        }
        let mut cookies = Vec::new();
        for p in params {
            let Some(name) = p["name"].as_str() else {
                continue;
            };
            let required = p["required"] == true;
            let value = self.param_value(p);
            match p["in"].as_str() {
                Some("path") => {
                    let name = var_name(name);
                    if valid_name(&name) {
                        r.vars.0.push(Variable {
                            name,
                            value: Some(value),
                            secret: false,
                        });
                    }
                }
                Some("query") => {
                    let kv = KeyValue::new(name, value);
                    r.query.0.push(if required { kv } else { kv.disabled() });
                }
                Some("header") => {
                    if ["accept", "content-type", "authorization"]
                        .contains(&name.to_ascii_lowercase().as_str())
                    {
                        continue;
                    }
                    let kv = KeyValue::new(name, value);
                    r.headers.0.push(if required { kv } else { kv.disabled() });
                }
                Some("cookie") => cookies.push(format!("{name}={value}")),
                _ => {}
            }
        }
        if !cookies.is_empty() {
            r.headers
                .0
                .push(KeyValue::new("Cookie", cookies.join("; ")));
        }
        let body = self.resolve(&op["requestBody"], 0);
        r.body = self.body(body, r.name.as_deref().unwrap_or_default());
        r
    }

    fn param_value(&mut self, p: &'a Value) -> String {
        let schema = self.resolve(&p["schema"], 0);
        let v = if let Some(e) = p.get("example") {
            e.clone()
        } else if let Some(e) = first_example(&p["examples"], self) {
            e
        } else {
            self.example(schema, 0)
        };
        literal(&match v {
            Value::String(s) => s,
            Value::Null => String::new(),
            Value::Array(a) => a.first().map(text_of).unwrap_or_default(),
            other => other.to_string(),
        })
    }

    fn body(&mut self, body: &'a Value, request: &str) -> Option<BodySpec> {
        let content = body["content"].as_object()?;
        let pick = |want: &dyn Fn(&str) -> bool| content.iter().find(|(ct, _)| want(ct));
        let (ct, media) = pick(&|ct| is_json_type(ct))
            .or_else(|| pick(&|ct| media_type(ct) == "application/x-www-form-urlencoded"))
            .or_else(|| pick(&|ct| media_type(ct) == "multipart/form-data"))
            .or_else(|| content.iter().next())?;
        let schema = self.resolve(&media["schema"], 0);
        let example = if let Some(e) = media.get("example") {
            e.clone()
        } else if let Some(e) = first_example(&media["examples"], self) {
            e
        } else {
            self.example(schema, 0)
        };
        let m = media_type(ct);
        if is_json_type(ct) {
            let text = serde_json::to_string_pretty(&example).unwrap_or_default();
            return Some(BodySpec::Json {
                content: Some(literal(&text)),
                file: None,
            });
        }
        if m == "application/x-www-form-urlencoded" || m == "multipart/form-data" {
            let fields: Vec<(String, String)> = example
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.clone(), literal(&text_of(v))))
                .collect();
            if m == "application/x-www-form-urlencoded" {
                return Some(BodySpec::Form {
                    fields: fields
                        .into_iter()
                        .map(|(k, v)| KeyValue::new(k, v))
                        .collect::<Vec<_>>()
                        .into(),
                });
            }
            let mut parts = Vec::new();
            for (k, v) in fields {
                let binary = matches!(
                    self.resolve(&schema["properties"][k.as_str()], 0)["format"].as_str(),
                    Some("binary" | "base64")
                );
                if binary {
                    self.imp.warn(format!(
                        "`{request}`: the file part `{k}` was left empty; point it at a file"
                    ));
                }
                parts.push(PartSpec {
                    name: k,
                    value: Some(if binary { String::new() } else { v }),
                    file: None,
                    filename: None,
                    content_type: None,
                    enabled: true,
                });
            }
            return Some(BodySpec::Multipart { parts });
        }
        if m.starts_with("text/") || m.ends_with("xml") || example.is_string() {
            return Some(BodySpec::Text {
                content: Some(literal(&text_of(&example))),
                file: None,
                content_type: Some(ct.clone()),
            });
        }
        self.imp.warn(format!(
            "`{request}`: the `{ct}` body was not imported; set it yourself"
        ));
        None
    }

    /// An example value generated from a schema.
    fn example(&mut self, schema: &'a Value, depth: usize) -> Value {
        let mut stack = Vec::new();
        self.example_in(schema, depth, &mut stack)
    }

    /// `stack` holds the `$ref`s being expanded: a schema that contains itself stops there.
    fn example_in(&mut self, schema: &'a Value, depth: usize, stack: &mut Vec<&'a str>) -> Value {
        if depth > MAX_DEPTH {
            return Value::Null;
        }
        if let Some(r) = schema.get("$ref").and_then(Value::as_str) {
            if stack.contains(&r) {
                return Value::Null;
            }
            stack.push(r);
            let target = self.resolve(schema, depth);
            let v = self.example_in(target, depth + 1, stack);
            stack.pop();
            return v;
        }
        let s = schema;
        for key in ["example", "default", "const"] {
            if let Some(v) = s.get(key) {
                return v.clone();
            }
        }
        if let Some(v) = s["examples"].get(0) {
            return v.clone();
        }
        if let Some(v) = s["enum"].get(0) {
            return v.clone();
        }
        if let Some(all) = s["allOf"].as_array() {
            let mut merged = Map::new();
            for part in all {
                match self.example_in(part, depth + 1, stack) {
                    Value::Object(o) => merged.extend(o),
                    other if merged.is_empty() => return other,
                    _ => {}
                }
            }
            return Value::Object(merged);
        }
        for key in ["oneOf", "anyOf"] {
            if let Some(first) = s[key].get(0) {
                return self.example_in(first, depth + 1, stack);
            }
        }
        let ty = match &s["type"] {
            Value::String(t) => t.as_str(),
            Value::Array(types) => types
                .iter()
                .filter_map(Value::as_str)
                .find(|t| *t != "null")
                .unwrap_or("null"),
            _ if s["properties"].is_object() => "object",
            _ if !s["items"].is_null() => "array",
            _ => "",
        };
        match ty {
            "object" => {
                let mut out = Map::new();
                for (k, prop) in s["properties"].as_object().into_iter().flatten() {
                    let prop_schema = self.resolve(prop, depth);
                    if prop_schema["readOnly"] == true {
                        continue;
                    }
                    out.insert(k.clone(), self.example_in(prop, depth + 1, stack));
                }
                Value::Object(out)
            }
            "array" => {
                let item = self.example_in(&s["items"], depth + 1, stack);
                if item.is_null() {
                    json!([])
                } else {
                    json!([item])
                }
            }
            "string" => Value::String(
                match s["format"].as_str() {
                    Some("date-time") => "2024-01-01T00:00:00Z",
                    Some("date") => "2024-01-01",
                    Some("time") => "12:00:00",
                    Some("email") => "user@example.com",
                    Some("uuid") => "00000000-0000-0000-0000-000000000000",
                    Some("uri" | "url") => "https://example.com",
                    Some("hostname") => "example.com",
                    Some("ipv4") => "192.0.2.1",
                    Some("binary" | "byte" | "base64" | "password") => "",
                    _ => "string",
                }
                .to_owned(),
            ),
            "integer" => json!(0),
            "number" => json!(0.0),
            "boolean" => json!(true),
            _ => Value::Null,
        }
    }
}

/// The `value` of the first entry of an `examples` map (resolving references).
fn first_example<'a>(examples: &'a Value, spec: &mut Spec<'a>) -> Option<Value> {
    let (_, first) = examples.as_object()?.iter().next()?;
    let e = spec.resolve(first, 0);
    e.get("value").cloned()
}

fn plain(imp: &mut Imported, name: &str) {
    if imp.collection.vars.get(name).is_none() {
        imp.collection.vars.set(name, "");
    }
}

/// A JSON value as text: strings unquoted, the rest as JSON.
fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}
