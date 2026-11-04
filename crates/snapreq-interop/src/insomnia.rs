//! Insomnia exports: v4 (JSON, `_type: export`) and v5 (YAML, `type: collection.insomnia.rest/5.0`).
//!
//! Template tags are mapped where SnapRequest has an equivalent: `{{ _.name }}` becomes `{{name}}`,
//! `{% uuid %}` `{{$uuid}}` and `{% now %}` a timestamp. Other tags are kept as text, with a warning.

use std::collections::HashMap;

use serde_json::Value;
use snapreq_collection::vars::valid_name;
use snapreq_collection::{
    AuthSpec, BodySpec, FolderFile, KeyValues, OAuth2Spec, PartSpec, RequestFile, Variable,
    Variables,
};
use snapreq_core::model::{ApiKeyLocation, KeyValue, OAuth2Grant};

use crate::error::{Error, Result};
use crate::model::{Imported, Node};
use crate::util::{header, protect_headers, split_query, text_body, tidy_headers};

const FORMAT: &str = "Insomnia";

pub(crate) fn import(text: &str) -> Result<Imported> {
    let doc = crate::parse_document(text).map_err(|e| Error::invalid(FORMAT, e))?;
    if doc["_type"] == "export" {
        v4(&doc, text)
    } else if doc["type"]
        .as_str()
        .is_some_and(|t| t.starts_with("collection.insomnia.rest/"))
    {
        v5(&doc, text)
    } else {
        Err(Error::invalid(FORMAT, "not an Insomnia v4 or v5 export"))
    }
}

fn v4(doc: &Value, text: &str) -> Result<Imported> {
    let resources = doc["resources"]
        .as_array()
        .ok_or_else(|| Error::invalid(FORMAT, "no `resources` list"))?;
    let of_type = |t: &'static str| resources.iter().filter(move |r| r["_type"] == t);
    let workspaces: Vec<&Value> = of_type("workspace").collect();
    let workspace = workspaces.first().copied();
    let name = workspace
        .and_then(|w| w["name"].as_str())
        .unwrap_or("Imported");
    let mut imp = Imported::new(name);
    imp.reserve(text);
    if workspaces.len() > 1 {
        imp.warn("the export holds several workspaces; only the first was imported");
    }
    let root_id = workspace
        .and_then(|w| w["_id"].as_str())
        .unwrap_or_default()
        .to_owned();

    // Children by parent, ordered by Insomnia's sort key.
    let mut children: HashMap<&str, Vec<&Value>> = HashMap::new();
    for r in resources {
        if let Some(parent) = r["parentId"].as_str() {
            children.entry(parent).or_default().push(r);
        }
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| {
            let key = |v: &Value| v["metaSortKey"].as_f64().unwrap_or(0.0);
            key(a).total_cmp(&key(b))
        });
    }

    // Environments: the base environment (child of the workspace) holds collection variables,
    // its sub-environments are environments.
    let mut skipped = 0;
    for base in children.get(root_id.as_str()).into_iter().flatten() {
        if base["_type"] != "environment" {
            continue;
        }
        imp.collection.vars = env_vars(&mut imp, &base["data"]);
        let base_id = base["_id"].as_str().unwrap_or_default();
        for sub in children.get(base_id).into_iter().flatten() {
            if sub["_type"] == "environment" {
                let vars = env_vars(&mut imp, &sub["data"]);
                imp.add_environment(sub["name"].as_str().unwrap_or("environment"), vars);
            }
        }
        break;
    }
    imp.items = v4_items(&mut imp, &children, &root_id, &mut skipped);
    if skipped > 0 {
        imp.warn(format!(
            "{skipped} gRPC, WebSocket or GraphQL-subscription item(s) were skipped"
        ));
    }
    Ok(imp)
}

fn v4_items(
    imp: &mut Imported,
    children: &HashMap<&str, Vec<&Value>>,
    parent: &str,
    skipped: &mut usize,
) -> Vec<Node> {
    let mut out = Vec::new();
    for r in children.get(parent).into_iter().flatten() {
        match r["_type"].as_str() {
            Some("request_group") => {
                let id = r["_id"].as_str().unwrap_or_default();
                let items = v4_items(imp, children, id, skipped);
                let folder = FolderFile {
                    description: non_empty(&r["description"]),
                    vars: env_vars(imp, &r["environment"]),
                    auth: auth(imp, &r["authentication"]),
                    headers: kvs(&r["headers"]),
                    ..FolderFile::default()
                };
                out.push(Node::Folder {
                    name: r["name"].as_str().unwrap_or("folder").to_owned(),
                    file: folder,
                    items,
                });
            }
            Some("request") => out.push(Node::Request(request(imp, r))),
            Some("grpc_request" | "websocket_request") => *skipped += 1,
            _ => {}
        }
    }
    out
}

fn v5(doc: &Value, text: &str) -> Result<Imported> {
    let mut imp = Imported::new(doc["name"].as_str().unwrap_or("Imported"));
    imp.reserve(text);
    let envs = &doc["environments"];
    imp.collection.vars = env_vars(&mut imp, &envs["data"]);
    for sub in envs["subEnvironments"].as_array().into_iter().flatten() {
        let vars = env_vars(&mut imp, &sub["data"]);
        imp.add_environment(sub["name"].as_str().unwrap_or("environment"), vars);
    }
    let mut skipped = 0;
    imp.items = v5_items(&mut imp, &doc["collection"], &mut skipped);
    if skipped > 0 {
        imp.warn(format!("{skipped} gRPC or WebSocket item(s) were skipped"));
    }
    Ok(imp)
}

fn v5_items(imp: &mut Imported, list: &Value, skipped: &mut usize) -> Vec<Node> {
    let mut items: Vec<&Value> = list.as_array().into_iter().flatten().collect();
    items.sort_by(|a, b| {
        let key = |v: &Value| v["meta"]["sortKey"].as_f64().unwrap_or(0.0);
        key(a).total_cmp(&key(b))
    });
    let mut out = Vec::new();
    for item in items {
        if item["children"].is_array() {
            let children = v5_items(imp, &item["children"], skipped);
            let folder = FolderFile {
                description: non_empty(&item["meta"]["description"]),
                vars: env_vars(imp, &item["environment"]),
                auth: auth(imp, &item["authentication"]),
                headers: kvs(&item["headers"]),
                ..FolderFile::default()
            };
            out.push(Node::Folder {
                name: item["name"].as_str().unwrap_or("folder").to_owned(),
                file: folder,
                items: children,
            });
        } else if item["url"].is_string() && item["protoFileId"].is_null() {
            out.push(Node::Request(request(imp, item)));
        } else {
            *skipped += 1;
        }
    }
    out
}

fn non_empty(v: &Value) -> Option<String> {
    v.as_str()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// Insomnia template syntax to SnapRequest placeholders.
fn tpl(imp: &mut Imported, s: &str) -> String {
    if !s.contains("{{") && !s.contains("{%") {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find(['{']) {
        out.push_str(&rest[..i]);
        let tail = &rest[i..];
        if let Some(inner) = tail.strip_prefix("{{")
            && let Some(end) = inner.find("}}")
        {
            let expr = inner[..end].trim();
            let name = expr.strip_prefix("_.").unwrap_or(expr).trim();
            let name = name
                .strip_prefix("_['")
                .and_then(|n| n.strip_suffix("']"))
                .unwrap_or(name);
            out.push_str(&format!("{{{{{name}}}}}"));
            rest = &inner[end + 2..];
        } else if let Some(inner) = tail.strip_prefix("{%")
            && let Some(end) = inner.find("%}")
        {
            let tag = inner[..end].trim();
            let word = tag.split_whitespace().next().unwrap_or_default();
            match word {
                "uuid" => out.push_str("{{$uuid}}"),
                "now" if tag.contains("unix") => out.push_str("{{$timestamp}}"),
                "now" => out.push_str("{{$isoTimestamp}}"),
                _ => {
                    imp.warn(format!(
                        "the Insomnia template tag `{word}` has no SnapRequest equivalent; it was kept as text"
                    ));
                    out.push_str(&tail[..end + 4]);
                }
            }
            rest = &inner[end + 2..];
        } else {
            out.push('{');
            rest = &tail[1..];
        }
    }
    out.push_str(rest);
    out
}

/// Environment data (nested objects become dotted names: `{{ _.api.host }}` is `{{api.host}}`).
fn env_vars(imp: &mut Imported, data: &Value) -> Variables {
    fn walk(imp: &mut Imported, prefix: &str, v: &Value, out: &mut Variables) {
        for (k, v) in v.as_object().into_iter().flatten() {
            let name = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match v {
                Value::Object(_) => walk(imp, &name, v, out),
                _ if !valid_name(&name) => imp.warn(format!(
                    "the variable `{name}` was skipped: names may use letters, digits, `_`, `-` and `.`"
                )),
                other => {
                    let text = match other {
                        Value::String(s) => tpl(imp, s),
                        Value::Null => String::new(),
                        x => x.to_string(),
                    };
                    out.0.push(Variable {
                        name,
                        value: Some(text),
                        secret: false,
                    });
                }
            }
        }
    }
    let mut out = Variables::default();
    walk(imp, "", data, &mut out);
    out
}

fn kvs(v: &Value) -> KeyValues {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|h| {
            let name = h["name"].as_str()?;
            if name.is_empty() {
                return None;
            }
            let kv = KeyValue::new(name, h["value"].as_str().unwrap_or_default());
            Some(if h["disabled"] == true {
                kv.disabled()
            } else {
                kv
            })
        })
        .collect::<Vec<_>>()
        .into()
}

fn tpl_kvs(imp: &mut Imported, mut kv: KeyValues) -> KeyValues {
    for e in &mut kv.0 {
        e.name = tpl(imp, &e.name);
        e.value = tpl(imp, &e.value);
    }
    kv
}

fn request(imp: &mut Imported, r: &Value) -> RequestFile {
    let raw_url = tpl(imp, r["url"].as_str().unwrap_or_default());
    let (url, mut query) = split_query(&raw_url);
    query.extend(tpl_kvs(imp, kvs(&r["parameters"])).0);
    let method = r["method"].as_str().unwrap_or("GET");
    let mut f = RequestFile::new(
        if http::Method::from_bytes(method.as_bytes()).is_ok() {
            method
        } else {
            "GET"
        },
        if url.trim().is_empty() {
            "{{base_url}}".to_owned()
        } else {
            url
        },
    );
    f.name = Some(r["name"].as_str().unwrap_or("request").to_owned());
    f.description = non_empty(&r["description"]).or_else(|| non_empty(&r["meta"]["description"]));
    f.query = query.into();
    f.headers = tpl_kvs(imp, kvs(&r["headers"]));
    f.body = body(imp, &r["body"], &f.headers);
    f.auth = auth(imp, &r["authentication"]);
    protect_headers(imp, &mut f.headers, &mut f.auth);
    tidy_headers(&mut f.headers, f.body.as_ref());
    f
}

fn body(imp: &mut Imported, b: &Value, headers: &KeyValues) -> Option<BodySpec> {
    let mime = b["mimeType"]
        .as_str()
        .map(str::to_owned)
        .or_else(|| header(headers, "content-type").map(str::to_owned));
    let mime_l = mime.as_deref().map(crate::util::media_type);
    match mime_l.as_deref() {
        Some("application/x-www-form-urlencoded") => Some(BodySpec::Form {
            fields: tpl_kvs(imp, kvs(&b["params"])),
        }),
        Some("multipart/form-data") => {
            let parts = b["params"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    let name = p["name"].as_str()?.to_owned();
                    let enabled = p["disabled"] != true;
                    if p["type"] == "file" {
                        let path = p["fileName"].as_str().unwrap_or_default();
                        if path.is_empty() {
                            return None;
                        }
                        imp.warn(format!(
                            "the file part `{name}` refers to `{path}`; copy the file next to the request"
                        ));
                        return Some(PartSpec {
                            name,
                            value: None,
                            file: Some(path.into()),
                            filename: None,
                            content_type: None,
                            enabled,
                        });
                    }
                    let value = tpl(imp, p["value"].as_str().unwrap_or_default());
                    Some(PartSpec {
                        name,
                        value: Some(value),
                        file: None,
                        filename: None,
                        content_type: None,
                        enabled,
                    })
                })
                .collect();
            Some(BodySpec::Multipart { parts })
        }
        Some("application/octet-stream") if b["fileName"].is_string() => {
            let path = b["fileName"].as_str().unwrap_or_default();
            imp.warn(format!(
                "the body file `{path}` must be copied next to the request"
            ));
            Some(BodySpec::File {
                file: path.into(),
                content_type: None,
            })
        }
        Some("application/graphql") => {
            let text = b["text"].as_str().unwrap_or_default();
            Some(BodySpec::Json {
                content: Some(tpl(imp, text)),
                file: None,
            })
        }
        _ => {
            let text = b["text"].as_str()?;
            if text.is_empty() {
                return None;
            }
            let text = tpl(imp, text);
            Some(text_body(mime.as_deref(), &text))
        }
    }
}

fn auth(imp: &mut Imported, a: &Value) -> Option<AuthSpec> {
    if a["disabled"] == true {
        return None;
    }
    let s = |imp: &mut Imported, key: &str| tpl(imp, a[key].as_str().unwrap_or_default());
    Some(match a["type"].as_str()? {
        "none" => AuthSpec::None,
        "basic" => {
            let password = s(imp, "password");
            AuthSpec::Basic {
                username: s(imp, "username"),
                password: imp.secret("password", &password),
            }
        }
        "digest" => {
            let password = s(imp, "password");
            AuthSpec::Digest {
                username: s(imp, "username"),
                password: imp.secret("password", &password),
            }
        }
        "bearer" => {
            let token = s(imp, "token");
            let prefix = s(imp, "prefix");
            if !prefix.is_empty() && !prefix.eq_ignore_ascii_case("bearer") {
                imp.warn(format!(
                    "the bearer prefix `{prefix}` was replaced by `Bearer`"
                ));
            }
            AuthSpec::Bearer {
                token: imp.secret("token", &token),
            }
        }
        "apikey" => {
            let name = s(imp, "key");
            let value = s(imp, "value");
            AuthSpec::ApiKey {
                value: imp.secret(&name.to_ascii_lowercase().replace('-', "_"), &value),
                name,
                location: if a["addTo"] == "queryParams" {
                    ApiKeyLocation::Query
                } else {
                    ApiKeyLocation::Header
                },
            }
        }
        "oauth2" => {
            let grant = match a["grantType"].as_str() {
                Some("password") => OAuth2Grant::Password,
                Some("authorization_code") => OAuth2Grant::AuthorizationCode,
                Some("client_credentials") | None => OAuth2Grant::ClientCredentials,
                Some(other) => {
                    imp.warn(format!("the OAuth 2.0 grant `{other}` is not supported"));
                    return None;
                }
            };
            let secret = s(imp, "clientSecret");
            let password = s(imp, "password");
            AuthSpec::OAuth2(OAuth2Spec {
                grant,
                token_url: s(imp, "accessTokenUrl"),
                auth_url: Some(s(imp, "authorizationUrl")).filter(|u| !u.is_empty()),
                client_id: s(imp, "clientId"),
                client_secret: imp.secret("client_secret", &secret),
                scope: s(imp, "scope"),
                username: s(imp, "username"),
                password: imp.secret("password", &password),
                redirect_uri: None,
                client_auth: None,
                params: KeyValues::default(),
            })
        }
        "iam" => {
            let secret = s(imp, "secretAccessKey");
            let session = s(imp, "sessionToken");
            AuthSpec::AwsSigV4 {
                access_key: s(imp, "accessKeyId"),
                secret_key: imp.secret("aws_secret_key", &secret),
                session_token: imp.secret("aws_session_token", &session),
                region: s(imp, "region"),
                service: s(imp, "service"),
            }
        }
        other => {
            imp.warn(format!("Insomnia auth type `{other}` is not supported"));
            return None;
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates() {
        let mut imp = Imported::new("t");
        assert_eq!(
            tpl(
                &mut imp,
                "{{ _.base }}/x/{{ id }}?t={% now 'unix', '' %}&u={% uuid 'v4' %}"
            ),
            "{{base}}/x/{{id}}?t={{$timestamp}}&u={{$uuid}}"
        );
        assert_eq!(tpl(&mut imp, "{\"a\": 1}"), "{\"a\": 1}");
        assert_eq!(
            tpl(&mut imp, "{% response 'body', 'req_1', '$.id' %}"),
            "{% response 'body', 'req_1', '$.id' %}"
        );
        assert_eq!(imp.warnings.len(), 1);
    }
}
