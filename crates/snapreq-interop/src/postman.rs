//! Postman Collection v2.1 (and v2.0) and Postman environments.

use serde_json::Value;
use snapreq_collection::vars::valid_name;
use snapreq_collection::{
    AuthSpec, BodySpec, FolderFile, KeyValues, OAuth2Spec, PartSpec, RequestFile, ScriptSpec,
    Variable, Variables,
};
use snapreq_core::model::{ApiKeyLocation, ClientAuth, KeyValue, OAuth2Grant};

use crate::error::{Error, Result};
use crate::model::{Imported, Node};
use crate::util::{header, protect_headers, split_query, text_body, tidy_headers};

const FORMAT: &str = "Postman";

pub(crate) fn import(text: &str) -> Result<Imported> {
    let doc: Value =
        serde_json::from_str(text).map_err(|e| Error::invalid(FORMAT, e.to_string()))?;
    if !doc["item"].is_array() {
        return Err(Error::invalid(FORMAT, "no `item` list"));
    }
    let name = doc["info"]["name"].as_str().unwrap_or("Imported");
    let mut imp = Imported::new(name);
    imp.reserve(text);
    imp.collection.description = description(&doc["info"]["description"]);
    imp.collection.vars = variables(&mut imp, &doc["variable"], "collection");
    imp.collection.auth = auth(&mut imp, &doc["auth"]);
    imp.collection.script = scripts(&mut imp, &doc["event"], "the collection");
    imp.items = items(&mut imp, &doc["item"]);
    Ok(imp)
}

/// A Postman environment file: its name and variables.
pub(crate) fn environment(text: &str) -> Result<(String, Variables)> {
    let doc: Value = serde_json::from_str(text)
        .map_err(|e| Error::invalid("Postman environment", e.to_string()))?;
    let Some(values) = doc["values"].as_array() else {
        return Err(Error::invalid("Postman environment", "no `values` list"));
    };
    let mut vars = Variables::default();
    for v in values {
        let Some(name) = v["key"].as_str() else {
            continue;
        };
        if v["enabled"] == false || !valid_name(name) {
            continue;
        }
        vars.0.push(Variable {
            name: name.to_owned(),
            value: Some(fix_vars(&scalar(&v["value"]))),
            secret: v["type"] == "secret",
        });
    }
    Ok((
        doc["name"].as_str().unwrap_or("environment").to_owned(),
        vars,
    ))
}

/// Postman's `prerequest` and `test` events as a `[script]` table, translated (ADR-032).
fn scripts(imp: &mut Imported, events: &Value, place: &str) -> ScriptSpec {
    let mut spec = ScriptSpec::default();
    for ev in events.as_array().into_iter().flatten() {
        let exec = &ev["script"]["exec"];
        let src = match exec {
            Value::Array(lines) => lines
                .iter()
                .map(|l| l.as_str().unwrap_or_default())
                .collect::<Vec<_>>()
                .join("\n"),
            Value::String(s) => s.clone(),
            _ => continue,
        };
        if src.trim().is_empty() {
            continue;
        }
        let (slot, phase) = match ev["listen"].as_str() {
            Some("prerequest") => (&mut spec.pre_request, "pre-request"),
            Some("test") => (&mut spec.test, "test"),
            _ => continue,
        };
        let t = crate::postman_script::translate(&src);
        if t.disabled {
            imp.warn(format!(
                "{place}: the {phase} script could not be translated; it was kept as a comment"
            ));
        } else if t.skipped > 0 {
            imp.warn(format!(
                "{place}: {} line(s) of the {phase} script could not be translated and were commented out",
                t.skipped
            ));
        }
        match slot {
            Some(existing) => existing.push_str(&t.code),
            None => *slot = Some(t.code),
        }
    }
    spec
}

fn description(v: &Value) -> Option<String> {
    let text = v.as_str().or_else(|| v["content"].as_str())?.trim();
    (!text.is_empty()).then(|| text.to_owned())
}

/// A JSON scalar as text.
fn scalar(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

/// Postman's dynamic variables mapped to SnapRequest's.
fn fix_vars(s: &str) -> String {
    if !s.contains("{{$") {
        return s.to_owned();
    }
    s.replace("{{$guid}}", "{{$uuid}}")
        .replace("{{$randomUUID}}", "{{$uuid}}")
}

fn variables(imp: &mut Imported, list: &Value, place: &str) -> Variables {
    let mut vars = Variables::default();
    for v in list.as_array().into_iter().flatten() {
        let Some(name) = v["key"].as_str().or_else(|| v["id"].as_str()) else {
            continue;
        };
        if v["disabled"] == true {
            continue;
        }
        if !valid_name(name) {
            imp.warn(format!(
                "{place} variable `{name}` was skipped: names may use letters, digits, `_`, `-` and `.`"
            ));
            continue;
        }
        vars.0.push(Variable {
            name: name.to_owned(),
            value: Some(fix_vars(&scalar(&v["value"]))),
            secret: v["type"] == "secret",
        });
    }
    vars
}

fn items(imp: &mut Imported, list: &Value) -> Vec<Node> {
    let mut out = Vec::new();
    for item in list.as_array().into_iter().flatten() {
        let name = item["name"].as_str().unwrap_or("item").to_owned();
        if item["item"].is_array() {
            let folder = FolderFile {
                description: description(&item["description"]),
                auth: auth(imp, &item["auth"]),
                vars: variables(imp, &item["variable"], "folder"),
                script: scripts(imp, &item["event"], &format!("folder `{name}`")),
                ..FolderFile::default()
            };
            let children = items(imp, &item["item"]);
            out.push(Node::Folder {
                name,
                file: folder,
                items: children,
            });
        } else if !item["request"].is_null() {
            out.push(Node::Request(request(imp, &name, item)));
        }
    }
    out
}

/// The `{ key, value }` list of a v2.1 auth (v2.0 uses an object) as a lookup.
fn auth_params(v: &Value) -> impl Fn(&str) -> String + '_ {
    move |key: &str| match v {
        Value::Array(list) => list
            .iter()
            .find(|p| p["key"] == key)
            .map(|p| fix_vars(&scalar(&p["value"])))
            .unwrap_or_default(),
        Value::Object(map) => map
            .get(key)
            .map(|x| fix_vars(&scalar(x)))
            .unwrap_or_default(),
        _ => String::new(),
    }
}

fn auth(imp: &mut Imported, a: &Value) -> Option<AuthSpec> {
    let kind = a["type"].as_str()?;
    let p = auth_params(&a[kind]);
    Some(match kind {
        "noauth" => AuthSpec::None,
        "inherit" => return None,
        "basic" => AuthSpec::Basic {
            username: p("username"),
            password: imp.secret("password", &p("password")),
        },
        "digest" => AuthSpec::Digest {
            username: p("username"),
            password: imp.secret("password", &p("password")),
        },
        "bearer" => AuthSpec::Bearer {
            token: imp.secret("token", &p("token")),
        },
        "apikey" => {
            let name = p("key");
            let value = p("value");
            AuthSpec::ApiKey {
                value: imp.secret(&name.to_ascii_lowercase().replace('-', "_"), &value),
                name,
                location: if p("in") == "query" {
                    ApiKeyLocation::Query
                } else {
                    ApiKeyLocation::Header
                },
            }
        }
        "awsv4" => AuthSpec::AwsSigV4 {
            access_key: p("accessKey"),
            secret_key: imp.secret("aws_secret_key", &p("secretKey")),
            session_token: imp.secret("aws_session_token", &p("sessionToken")),
            region: non_empty(p("region"), "us-east-1"),
            service: non_empty(p("service"), "execute-api"),
        },
        "oauth2" => {
            let grant = match p("grant_type").as_str() {
                "password_credentials" => OAuth2Grant::Password,
                "authorization_code" | "authorization_code_with_pkce" => {
                    OAuth2Grant::AuthorizationCode
                }
                "client_credentials" | "" => OAuth2Grant::ClientCredentials,
                other => {
                    imp.warn(format!(
                        "Postman OAuth 2.0 grant `{other}` is not supported; client credentials were used"
                    ));
                    OAuth2Grant::ClientCredentials
                }
            };
            let token_url = p("accessTokenUrl");
            if token_url.is_empty() {
                imp.warn("a Postman OAuth 2.0 auth without a token URL was skipped");
                return None;
            }
            AuthSpec::OAuth2(OAuth2Spec {
                grant,
                token_url,
                auth_url: Some(p("authUrl")).filter(|u| !u.is_empty()),
                client_id: p("clientId"),
                client_secret: imp.secret("client_secret", &p("clientSecret")),
                scope: p("scope"),
                username: p("username"),
                password: imp.secret("password", &p("password")),
                redirect_uri: None,
                client_auth: (p("client_authentication") == "body").then_some(ClientAuth::Body),
                params: KeyValues::default(),
            })
        }
        other => {
            imp.warn(format!("Postman auth type `{other}` is not supported"));
            return None;
        }
    })
}

fn non_empty(s: String, default: &str) -> String {
    if s.is_empty() { default.to_owned() } else { s }
}

fn request(imp: &mut Imported, name: &str, item: &Value) -> RequestFile {
    let req = &item["request"];
    // v2.0 allows a bare URL string.
    let (method, url_value) = match req {
        Value::String(url) => ("GET", Value::String(url.clone())),
        _ => (req["method"].as_str().unwrap_or("GET"), req["url"].clone()),
    };
    let (url, query, path_vars) = url_of(&url_value);
    let mut r = RequestFile::new(method, fix_vars(&url));
    r.name = Some(name.to_owned());
    r.description = description(&item["description"]).or_else(|| description(&req["description"]));
    r.query = query.into();
    for (k, v) in path_vars {
        if valid_name(&k) {
            r.vars.0.push(Variable {
                name: k,
                value: Some(v),
                secret: false,
            });
        }
    }
    r.headers = headers(&req["header"]);
    r.body = body(imp, &req["body"], &r.headers);
    r.auth = auth(imp, &req["auth"]);
    r.script = scripts(imp, &item["event"], &format!("`{name}`"));
    protect_headers(imp, &mut r.headers, &mut r.auth);
    tidy_headers(&mut r.headers, r.body.as_ref());
    if let Some(m) = &r.method
        && http::Method::from_bytes(m.as_bytes()).is_err()
    {
        imp.warn(format!("`{name}`: invalid method `{m}`, GET was used"));
        r.method = None;
    }
    if r.url.trim().is_empty() {
        imp.warn(format!("`{name}` has no URL"));
        r.url = "{{base_url}}".into();
    }
    r
}

/// The URL without query, its query entries and its path variables (`:id` becomes `{{id}}`).
fn url_of(v: &Value) -> (String, Vec<KeyValue>, Vec<(String, String)>) {
    let raw = match v {
        Value::String(s) => s.clone(),
        _ => v["raw"].as_str().map(str::to_owned).unwrap_or_else(|| {
            let host = match &v["host"] {
                Value::Array(h) => h.iter().map(scalar).collect::<Vec<_>>().join("."),
                other => scalar(other),
            };
            let path = match &v["path"] {
                Value::Array(p) => p.iter().map(scalar).collect::<Vec<_>>().join("/"),
                other => scalar(other),
            };
            let proto = v["protocol"]
                .as_str()
                .map(|p| format!("{p}://"))
                .unwrap_or_default();
            format!("{proto}{host}/{path}")
        }),
    };
    let (base, mut query) = split_query(&raw);
    if let Some(list) = v["query"].as_array() {
        // The structured list keeps disabled entries and exact values.
        query = list
            .iter()
            .filter_map(|q| {
                let key = q["key"].as_str()?;
                let kv = KeyValue::new(fix_vars(key), fix_vars(&scalar(&q["value"])));
                Some(if q["disabled"] == true {
                    kv.disabled()
                } else {
                    kv
                })
            })
            .collect();
    } else {
        for q in &mut query {
            q.value = fix_vars(&q.value);
        }
    }
    let vars: Vec<(String, String)> = v["variable"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|x| {
            let key = x["key"].as_str()?;
            let value = scalar(&x["value"]);
            (!value.is_empty()).then(|| (key.to_owned(), fix_vars(&value)))
        })
        .collect();
    let base = path_variables(&base);
    (base, query, vars)
}

fn path_variables(url: &str) -> String {
    let (scheme, rest) = match url.split_once("://") {
        Some((s, r)) => (format!("{s}://"), r),
        None => (String::new(), url),
    };
    let segments: Vec<String> = rest
        .split('/')
        .enumerate()
        .map(|(i, seg)| match seg.strip_prefix(':') {
            Some(name) if i > 0 && !name.is_empty() && valid_name(name) => {
                format!("{{{{{name}}}}}")
            }
            _ => seg.to_owned(),
        })
        .collect();
    format!("{scheme}{}", segments.join("/"))
}

fn headers(v: &Value) -> KeyValues {
    match v {
        Value::Array(list) => list
            .iter()
            .filter_map(|h| {
                let kv = KeyValue::new(h["key"].as_str()?, fix_vars(&scalar(&h["value"])));
                Some(if h["disabled"] == true {
                    kv.disabled()
                } else {
                    kv
                })
            })
            .collect::<Vec<_>>()
            .into(),
        Value::String(text) => text
            .lines()
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| KeyValue::new(k.trim(), fix_vars(v.trim())))
            .collect::<Vec<_>>()
            .into(),
        _ => KeyValues::default(),
    }
}

fn body(imp: &mut Imported, b: &Value, headers: &KeyValues) -> Option<BodySpec> {
    if b["disabled"] == true {
        return None;
    }
    let ct = header(headers, "content-type");
    match b["mode"].as_str()? {
        "raw" => {
            let raw = fix_vars(b["raw"].as_str().unwrap_or_default());
            if raw.is_empty() {
                return None;
            }
            match b["options"]["raw"]["language"].as_str() {
                Some("json") if ct.is_none() => Some(BodySpec::Json {
                    content: Some(raw),
                    file: None,
                }),
                _ => Some(text_body(ct, &raw)),
            }
        }
        "urlencoded" => Some(BodySpec::Form {
            fields: pairs(&b["urlencoded"]),
        }),
        "formdata" => {
            let parts = b["formdata"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    let name = p["key"].as_str()?.to_owned();
                    let enabled = p["disabled"] != true;
                    let content_type = p["contentType"].as_str().map(str::to_owned);
                    if p["type"] == "file" {
                        let src = match &p["src"] {
                            Value::Array(a) => a.first().map(scalar).unwrap_or_default(),
                            other => scalar(other),
                        };
                        if src.is_empty() {
                            imp.warn(format!("the file part `{name}` has no file; it was skipped"));
                            return None;
                        }
                        imp.warn(format!(
                            "the file part `{name}` refers to `{src}`; copy the file next to the request"
                        ));
                        return Some(PartSpec {
                            name,
                            value: None,
                            file: Some(src.into()),
                            filename: None,
                            content_type,
                            enabled,
                        });
                    }
                    Some(PartSpec {
                        name,
                        value: Some(fix_vars(&scalar(&p["value"]))),
                        file: None,
                        filename: None,
                        content_type,
                        enabled,
                    })
                })
                .collect();
            Some(BodySpec::Multipart { parts })
        }
        "file" => {
            let src = scalar(&b["file"]["src"]);
            if src.is_empty() {
                return None;
            }
            imp.warn(format!(
                "the body file `{src}` must be copied next to the request"
            ));
            Some(BodySpec::File {
                file: src.into(),
                content_type: ct.map(str::to_owned),
            })
        }
        "graphql" => {
            let g = &b["graphql"];
            let variables: Value = match &g["variables"] {
                Value::String(s) if !s.trim().is_empty() => {
                    serde_json::from_str(s).unwrap_or(Value::Object(Default::default()))
                }
                Value::Object(o) => Value::Object(o.clone()),
                _ => Value::Object(Default::default()),
            };
            let content = serde_json::json!({
                "query": g["query"].as_str().unwrap_or_default(),
                "variables": variables,
            });
            Some(BodySpec::Json {
                content: Some(fix_vars(
                    &serde_json::to_string_pretty(&content).unwrap_or_default(),
                )),
                file: None,
            })
        }
        other => {
            imp.warn(format!("Postman body mode `{other}` is not supported"));
            None
        }
    }
}

fn pairs(v: &Value) -> KeyValues {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let kv = KeyValue::new(fix_vars(p["key"].as_str()?), fix_vars(&scalar(&p["value"])));
            Some(if p["disabled"] == true {
                kv.disabled()
            } else {
                kv
            })
        })
        .collect::<Vec<_>>()
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_variables_become_placeholders() {
        assert_eq!(
            path_variables("{{base}}/users/:id/posts/:post"),
            "{{base}}/users/{{id}}/posts/{{post}}"
        );
        assert_eq!(
            path_variables("http://a.test:8080/x"),
            "http://a.test:8080/x"
        );
    }
}
