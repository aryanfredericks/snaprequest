//! HTTP Archive (HAR 1.2) recordings, as saved by browsers and proxies.
//!
//! Static assets (images, scripts, styles, fonts, media) are skipped when the recording marks
//! them. With one origin it becomes `{{base_url}}`; with several, each origin gets a folder.
//! Recorded text is literal: `{{` is escaped so it is not read as a placeholder.

use serde_json::Value;
use snapreq_collection::{BodySpec, KeyValues, PartSpec, RequestFile};
use snapreq_core::model::KeyValue;

use crate::curl::title;
use crate::error::{Error, Result};
use crate::model::{Imported, Node};
use crate::util::{literal, media_type, protect_headers, text_body, tidy_headers};

const FORMAT: &str = "HAR";

/// Chrome's `_resourceType` values that are not API calls.
const ASSETS: [&str; 8] = [
    "image",
    "stylesheet",
    "script",
    "font",
    "media",
    "manifest",
    "texttrack",
    "document",
];

pub(crate) fn import(text: &str) -> Result<Imported> {
    let doc: Value =
        serde_json::from_str(text).map_err(|e| Error::invalid(FORMAT, e.to_string()))?;
    let entries = doc["log"]["entries"]
        .as_array()
        .ok_or_else(|| Error::invalid(FORMAT, "no `log.entries` list"))?;
    let mut imp = Imported::new("Recorded requests");
    imp.reserve(text);
    let mut skipped = 0;
    let mut wanted = Vec::new();
    for e in entries {
        let req = &e["request"];
        let Some(url) = req["url"].as_str() else {
            continue;
        };
        let asset = e["_resourceType"]
            .as_str()
            .is_some_and(|t| ASSETS.contains(&t));
        if asset || !(url.starts_with("http://") || url.starts_with("https://")) {
            skipped += 1;
            continue;
        }
        wanted.push((origin(url), req));
    }
    if skipped > 0 {
        imp.warn(format!(
            "{skipped} entr{} (static assets, data: or WebSocket URLs) were skipped",
            if skipped == 1 { "y" } else { "ies" }
        ));
    }
    let mut origins: Vec<String> = Vec::new();
    for (o, _) in &wanted {
        if !origins.contains(o) {
            origins.push(o.clone());
        }
    }
    if origins.len() == 1 {
        imp.collection.vars.set("base_url", origins[0].clone());
        for (o, req) in &wanted {
            let r = request(&mut imp, req, Some(o));
            imp.items.push(Node::Request(r));
        }
    } else {
        for o in &origins {
            let items = wanted
                .iter()
                .filter(|(x, _)| x == o)
                .map(|(_, req)| Node::Request(request(&mut imp, req, None)))
                .collect();
            let name = o
                .split_once("://")
                .map_or(o.as_str(), |(_, h)| h)
                .to_owned();
            imp.items.push(Node::Folder {
                name,
                file: Default::default(),
                items,
            });
        }
    }
    Ok(imp)
}

/// `https://host:port` of a URL.
fn origin(url: &str) -> String {
    let (scheme, rest) = url.split_once("://").unwrap_or(("http", url));
    let host = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    format!("{scheme}://{host}")
}

fn request(imp: &mut Imported, req: &Value, base: Option<&String>) -> RequestFile {
    let raw = req["url"].as_str().unwrap_or_default();
    let without_query = raw.split(['?', '#']).next().unwrap_or(raw);
    let url = match base {
        Some(b) => format!("{{{{base_url}}}}{}", literal(&without_query[b.len()..])),
        None => literal(without_query),
    };
    let method = req["method"].as_str().unwrap_or("GET");
    let method = if http::Method::from_bytes(method.as_bytes()).is_ok() {
        method
    } else {
        "GET"
    };
    let mut r = RequestFile::new(method, url);
    r.name = Some(title(method, without_query));
    r.query = pairs(&req["queryString"]);
    if r.query.is_empty() && raw.contains('?') {
        r.query = crate::util::split_query(raw)
            .1
            .into_iter()
            .map(|kv| KeyValue::new(literal(&kv.name), literal(&kv.value)))
            .collect::<Vec<_>>()
            .into();
    }
    r.headers = pairs(&req["headers"]);
    // Browsers record these; the client sets its own.
    r.headers.0.retain(|h| {
        !matches!(
            h.name.to_ascii_lowercase().as_str(),
            "accept-encoding" | "upgrade-insecure-requests"
        ) && !h.name.to_ascii_lowercase().starts_with("sec-")
    });
    let post = &req["postData"];
    let mime = post["mimeType"].as_str().filter(|m| !m.is_empty());
    let m = mime.map(media_type).unwrap_or_default();
    r.body = if m == "multipart/form-data" && post["params"].is_array() {
        let mut parts = Vec::new();
        for p in post["params"].as_array().into_iter().flatten() {
            let Some(name) = p["name"].as_str() else {
                continue;
            };
            if p["fileName"].is_string() {
                imp.warn(format!(
                    "the recorded file part `{name}` was left empty; point it at a file"
                ));
            }
            parts.push(PartSpec {
                name: name.to_owned(),
                value: Some(literal(p["value"].as_str().unwrap_or_default())),
                file: None,
                filename: None,
                content_type: p["contentType"].as_str().map(str::to_owned),
                enabled: true,
            });
        }
        Some(BodySpec::Multipart { parts })
    } else if m == "application/x-www-form-urlencoded" && post["params"].is_array() {
        Some(BodySpec::Form {
            fields: pairs(&post["params"]),
        })
    } else {
        match post["text"].as_str() {
            Some(t) if !t.is_empty() => Some(text_body(mime, &literal(t))),
            _ => None,
        }
    };
    protect_headers(imp, &mut r.headers, &mut r.auth);
    tidy_headers(&mut r.headers, r.body.as_ref());
    r
}

fn pairs(v: &Value) -> KeyValues {
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            Some(KeyValue::new(
                literal(p["name"].as_str()?),
                literal(p["value"].as_str().unwrap_or_default()),
            ))
        })
        .collect::<Vec<_>>()
        .into()
}
