//! Exchanges as an HTTP Archive (HAR 1.2), for browser dev tools, proxies and HAR viewers.
//!
//! Secret values are replaced by `<redacted>`, and so are the values of credential headers
//! (`Authorization`, `Cookie`, `Set-Cookie`, API key headers) whatever they hold: a HAR file is
//! meant to be shared.

use serde_json::{Value, json};
use snapreq_core::model::{Body, Header, PartValue};
use snapreq_core::{Request, Response};

const REDACTED: &str = "<redacted>";

/// One request and what came back.
#[derive(Debug, Clone, Copy)]
pub struct Exchange<'a> {
    pub request: &'a Request,
    /// `None` when sending failed.
    pub response: Option<&'a Response>,
    /// When the request started, in Unix milliseconds.
    pub started_ms: i64,
}

/// A neutral entry both sources are converted to.
struct Entry {
    started_ms: i64,
    method: String,
    url: String,
    version: String,
    headers: Vec<(String, String)>,
    post: Option<Value>,
    response: Option<ResponseEntry>,
    total_ms: f64,
    timings: Value,
}

struct ResponseEntry {
    status: u16,
    headers: Vec<(String, String)>,
    /// Body text, or base64 with `encoding`.
    text: String,
    base64: bool,
    size: usize,
}

fn is_credential(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "authorization" | "proxy-authorization" | "cookie" | "set-cookie"
    ) || crate::util::is_credential_header(&n)
}

/// Replaces every secret (longest first) with `<redacted>`.
fn hide(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_owned();
    for s in secrets {
        if !s.is_empty() && out.contains(s.as_str()) {
            out = out.replace(s.as_str(), REDACTED);
        }
    }
    out
}

fn header_pairs<'h>(
    headers: impl Iterator<Item = (&'h str, &'h str)>,
    secrets: &[String],
) -> Vec<(String, String)> {
    headers
        .map(|(n, v)| {
            let value = if is_credential(n) {
                REDACTED.to_owned()
            } else {
                hide(v, secrets)
            };
            (n.to_owned(), value)
        })
        .collect()
}

fn iso(ms: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp_nanos(i128::from(ms) * 1_000_000)
        .ok()
        .and_then(|t| {
            t.format(&time::format_description::well_known::Rfc3339)
                .ok()
        })
        .unwrap_or_else(|| "1970-01-01T00:00:00Z".into())
}

fn post_data(body: &Body, secrets: &[String]) -> Option<Value> {
    Some(match body {
        Body::None => return None,
        Body::Text {
            content,
            content_type,
        } => json!({
            "mimeType": content_type.as_deref().unwrap_or("text/plain; charset=utf-8"),
            "text": hide(content, secrets),
        }),
        Body::Json { content } => {
            json!({ "mimeType": "application/json", "text": hide(content, secrets) })
        }
        Body::Form { fields } => {
            let enabled: Vec<_> = fields.iter().filter(|f| f.enabled).collect();
            let text = enabled
                .iter()
                .map(|f| {
                    format!(
                        "{}={}",
                        crate::util::form_encode(&f.name),
                        crate::util::form_encode(&hide(&f.value, secrets))
                    )
                })
                .collect::<Vec<_>>()
                .join("&");
            json!({
                "mimeType": "application/x-www-form-urlencoded",
                "text": text,
                "params": enabled.iter().map(|f| json!({ "name": f.name, "value": hide(&f.value, secrets) })).collect::<Vec<_>>(),
            })
        }
        Body::Multipart { parts } => json!({
            "mimeType": "multipart/form-data",
            "text": "",
            "params": parts.iter().filter(|p| p.enabled).map(|p| match &p.value {
                PartValue::Text { value } => json!({ "name": p.name, "value": hide(value, secrets) }),
                PartValue::File { path, filename, content_type } => {
                    let name = filename.clone().unwrap_or_else(|| {
                        path.file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default()
                    });
                    let mut v = json!({ "name": p.name, "fileName": name });
                    if let Some(ct) = content_type {
                        v["contentType"] = json!(ct);
                    }
                    v
                }
            }).collect::<Vec<_>>(),
        }),
        Body::File { path, content_type } => json!({
            "mimeType": content_type.as_deref().unwrap_or("application/octet-stream"),
            "text": "",
            "comment": format!("sent from the file {}", path.display()),
        }),
    })
}

fn ms(d: Option<std::time::Duration>) -> f64 {
    d.map_or(-1.0, |d| (d.as_secs_f64() * 1_000_000.0).round() / 1000.0)
}

/// HAR timings from SnapRequest's (`*_ms` keys as stored, or durations).
fn timings_json(dns: f64, connect: f64, tls: f64, send: f64, wait: f64, receive: f64) -> Value {
    // HAR's `connect` includes `ssl`.
    let connect = if connect >= 0.0 && tls >= 0.0 {
        connect + tls
    } else {
        connect
    };
    json!({
        "blocked": -1,
        "dns": dns,
        "connect": connect,
        "ssl": tls,
        "send": send.max(0.0),
        "wait": wait.max(0.0),
        "receive": receive.max(0.0),
    })
}

impl Entry {
    fn from_exchange(x: &Exchange<'_>) -> Self {
        let req = x.request;
        let mut secrets = req.secrets();
        secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
        let url = req
            .effective_url()
            .map_or_else(|_| req.url.clone(), |u| u.to_string());
        let mut headers = header_pairs(
            req.enabled_headers()
                .map(|h| (h.name.as_str(), h.value.as_str())),
            &secrets,
        );
        // Auth the engine added as a header.
        let auth_header = match &req.auth {
            snapreq_core::model::Auth::Basic { .. }
            | snapreq_core::model::Auth::Bearer { .. }
            | snapreq_core::model::Auth::OAuth2(_)
            | snapreq_core::model::Auth::AwsSigV4 { .. } => Some("Authorization".to_owned()),
            snapreq_core::model::Auth::ApiKey {
                name,
                location: snapreq_core::model::ApiKeyLocation::Header,
                ..
            } => Some(name.clone()),
            _ => None,
        };
        if let Some(name) = auth_header
            && !headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name))
        {
            headers.push((name, REDACTED.into()));
        }
        if let Some(ct) = req.body.default_content_type()
            && !headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        {
            headers.push(("Content-Type".into(), ct.to_owned()));
        }
        let response = x.response.map(|r| {
            let (text, base64) = match std::str::from_utf8(&r.body) {
                Ok(t) => (hide(t, &secrets), false),
                Err(_) => {
                    use base64::Engine as _;
                    (
                        base64::engine::general_purpose::STANDARD.encode(&r.body),
                        true,
                    )
                }
            };
            ResponseEntry {
                status: r.status,
                headers: header_pairs(
                    r.headers
                        .iter()
                        .map(|h: &Header| (h.name.as_str(), h.value.as_str())),
                    &secrets,
                ),
                text,
                base64,
                size: r.body.len(),
            }
        });
        let t = x.response.map(|r| r.timings.clone());
        Self {
            started_ms: x.started_ms,
            method: req.method.as_str().to_owned(),
            url: hide(&url, &secrets),
            version: x.response.map_or_else(
                || "HTTP/1.1".to_owned(),
                |r| {
                    serde_json::to_value(r.version)
                        .ok()
                        .and_then(|v| v.as_str().map(str::to_owned))
                        .unwrap_or_else(|| "HTTP/1.1".into())
                },
            ),
            headers,
            post: post_data(&req.body, &secrets),
            response,
            total_ms: t.as_ref().map_or(0.0, |t| ms(Some(t.total))),
            timings: t.map_or_else(
                || timings_json(-1.0, -1.0, -1.0, 0.0, 0.0, 0.0),
                |t| {
                    timings_json(
                        ms(t.dns),
                        ms(t.connect),
                        ms(t.tls),
                        ms(t.send),
                        ms(t.wait),
                        ms(t.download),
                    )
                },
            ),
        }
    }

    /// From a stored history entry (already redacted by the server).
    fn from_history(started_ms: i64, request: &Value, response: Option<&Value>) -> Self {
        let pairs = |v: &Value| -> Vec<(String, String)> {
            let list: Vec<(String, String)> = v
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|h| {
                    Some((
                        h["name"].as_str()?.to_owned(),
                        h["value"].as_str().unwrap_or_default().to_owned(),
                    ))
                })
                .collect();
            header_pairs(list.iter().map(|(n, v)| (n.as_str(), v.as_str())), &[])
        };
        let body: Body = serde_json::from_value(request["body"].clone()).unwrap_or_default();
        let mut headers = pairs(&request["headers"]);
        if let Some(ct) = body.default_content_type()
            && !headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("content-type"))
        {
            headers.push(("Content-Type".into(), ct.to_owned()));
        }
        let t = response.map(|r| &r["timings"]);
        let f = |key: &str| t.and_then(|t| t[key].as_f64()).unwrap_or(-1.0);
        Self {
            started_ms,
            method: request["method"].as_str().unwrap_or("GET").to_owned(),
            url: request["url"].as_str().unwrap_or_default().to_owned(),
            version: response
                .and_then(|r| r["version"].as_str())
                .unwrap_or("HTTP/1.1")
                .to_owned(),
            headers,
            post: post_data(&body, &[]),
            response: response.filter(|r| r["status"].is_u64()).map(|r| {
                let data = r["body"]["data"].as_str().unwrap_or_default().to_owned();
                let base64 = r["body"]["encoding"] == "base64";
                ResponseEntry {
                    status: r["status"]
                        .as_u64()
                        .and_then(|s| u16::try_from(s).ok())
                        .unwrap_or(0),
                    headers: pairs(&r["headers"]),
                    size: r["sizes"]["body"]
                        .as_u64()
                        .map_or(data.len(), |n| n as usize),
                    text: data,
                    base64,
                }
            }),
            total_ms: f("total_ms").max(0.0),
            timings: timings_json(
                f("dns_ms"),
                f("connect_ms"),
                f("tls_ms"),
                f("send_ms"),
                f("wait_ms"),
                f("download_ms"),
            ),
        }
    }

    fn json(&self) -> Value {
        let (path_query, query) = crate::util::split_query(&self.url);
        let _ = path_query;
        let headers = |h: &[(String, String)]| -> Vec<Value> {
            h.iter()
                .map(|(n, v)| json!({ "name": n, "value": v }))
                .collect()
        };
        let cookies = |h: &[(String, String)], name: &str| -> Vec<Value> {
            if h.iter().any(|(n, _)| n.eq_ignore_ascii_case(name)) {
                vec![json!({ "name": "(redacted)", "value": REDACTED })]
            } else {
                Vec::new()
            }
        };
        let mut request = json!({
            "method": self.method,
            "url": self.url,
            "httpVersion": self.version,
            "cookies": cookies(&self.headers, "cookie"),
            "headers": headers(&self.headers),
            "queryString": query.iter().map(|q| json!({ "name": q.name, "value": q.value })).collect::<Vec<_>>(),
            "headersSize": -1,
            "bodySize": self.post.as_ref().and_then(|p| p["text"].as_str()).map_or(0, str::len),
        });
        if let Some(p) = &self.post {
            request["postData"] = p.clone();
        }
        let response = match &self.response {
            Some(r) => {
                let mime = r
                    .headers
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case("content-type"))
                    .map_or("", |(_, v)| v.as_str());
                let mut content = json!({ "size": r.size, "mimeType": mime, "text": r.text });
                if r.base64 {
                    content["encoding"] = json!("base64");
                }
                let location = r
                    .headers
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case("location"))
                    .map_or("", |(_, v)| v.as_str());
                json!({
                    "status": r.status,
                    "statusText": http::StatusCode::from_u16(r.status).ok().and_then(|s| s.canonical_reason()).unwrap_or(""),
                    "httpVersion": self.version,
                    "cookies": cookies(&r.headers, "set-cookie"),
                    "headers": headers(&r.headers),
                    "content": content,
                    "redirectURL": location,
                    "headersSize": -1,
                    "bodySize": r.size,
                })
            }
            // HAR has no place for errors: status 0, like browsers record failed requests.
            None => json!({
                "status": 0,
                "statusText": "",
                "httpVersion": self.version,
                "cookies": [],
                "headers": [],
                "content": { "size": 0, "mimeType": "" },
                "redirectURL": "",
                "headersSize": -1,
                "bodySize": -1,
            }),
        };
        json!({
            "startedDateTime": iso(self.started_ms),
            "time": self.total_ms,
            "request": request,
            "response": response,
            "cache": {},
            "timings": self.timings,
        })
    }
}

fn document(entries: &[Entry]) -> Value {
    json!({
        "log": {
            "version": "1.2",
            "creator": { "name": "SnapRequest", "version": snapreq_core::VERSION },
            "entries": entries.iter().map(Entry::json).collect::<Vec<_>>(),
        }
    })
}

/// A HAR document of exchanges made by this process (CLI runs).
pub fn from_exchanges(exchanges: &[Exchange<'_>]) -> Value {
    document(
        &exchanges
            .iter()
            .map(Entry::from_exchange)
            .collect::<Vec<_>>(),
    )
}

/// A HAR document of stored history entries: `(created_at_ms, request, response)` as the server
/// keeps them (already redacted).
pub fn from_history(entries: &[(i64, &Value, Option<&Value>)]) -> Value {
    document(
        &entries
            .iter()
            .map(|(t, req, resp)| Entry::from_history(*t, req, *resp))
            .collect::<Vec<_>>(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapreq_core::model::{Auth, KeyValue};

    #[test]
    fn an_exchange_without_secrets() {
        let mut req = Request::new(http::Method::POST, "https://a.test/x?k=v");
        req.headers.push(KeyValue::new("Cookie", "sid=1"));
        req.headers
            .push(KeyValue::new("X-Note", "has s3cr3t inside"));
        req.body = Body::Json {
            content: "{\"t\":\"s3cr3t\"}".into(),
        };
        req.auth = Auth::Bearer {
            token: "s3cr3t".into(),
        };
        let doc = from_exchanges(&[Exchange {
            request: &req,
            response: None,
            started_ms: 1_700_000_000_000,
        }]);
        let text = doc.to_string();
        assert!(
            !text.contains("s3cr3t") && !text.contains("sid=1"),
            "{text}"
        );
        let e = &doc["log"]["entries"][0];
        assert_eq!(e["startedDateTime"], "2023-11-14T22:13:20Z");
        assert_eq!(e["request"]["queryString"][0]["name"], "k");
        assert_eq!(e["request"]["postData"]["mimeType"], "application/json");
        assert_eq!(e["response"]["status"], 0);
        let names: Vec<_> = e["request"]["headers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|h| h["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Cookie", "X-Note", "Authorization", "Content-Type"]);
    }
}
