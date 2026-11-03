//! `--format json`: one machine-readable document per invocation.
//!
//! Schema version 1 (documented in `docs/CLI.md`):
//! - `format`: always `1`
//! - `request`: `{ method, url }` (secrets removed from the URL)
//! - `response`: the core `Response` (status, version, url, headers, body, timings, sizes, trace)
//!   or `null`
//! - `body_file`: path the body was saved to with `-o`, else absent (the body is then empty)
//! - `error`: `{ kind, message, phase?, timed_out?, trace? }` or `null`

use std::path::Path;

use serde_json::{Value, json};
use snapreq_core::{Error, Request, Response};

/// Removes the request's secrets from a URL shown in the output.
fn scrub(req: &Request, url: &str) -> String {
    let mut url = url.to_owned();
    let mut secrets = req.secrets();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    for secret in secrets.iter().filter(|s| !s.is_empty()) {
        // Also the form a value takes inside a query string.
        let encoded: String = url::form_urlencoded::byte_serialize(secret.as_bytes()).collect();
        url = url.replace(secret.as_str(), "<redacted>");
        url = url.replace(&encoded, "<redacted>");
    }
    url
}

fn request_summary(req: &Request) -> Value {
    let url = req
        .effective_url()
        .map_or_else(|_| req.url.clone(), |u| u.to_string());
    json!({ "method": req.method.as_str(), "url": scrub(req, &url) })
}

/// Document for a received response.
pub(crate) fn success(req: &Request, resp: &Response, body_file: Option<&Path>) -> Value {
    let mut doc = json!({
        "format": 1,
        "request": request_summary(req),
        "response": resp,
        "error": null,
    });
    // The body is returned as received; everything else must not show secrets.
    doc["response"]["url"] = json!(scrub(req, &resp.url));
    if let Some(path) = body_file {
        doc["body_file"] = json!(path.display().to_string());
        doc["response"]["body"] = json!({ "encoding": "utf8", "data": "" });
    }
    doc
}

/// Document for a failure. `request` is `None` when the arguments could not even be
/// turned into a request.
pub(crate) fn failure(req: Option<&Request>, error: &Error) -> Value {
    let error_doc = match error {
        Error::Execution(e) => json!({
            "kind": "execution",
            "message": e.message,
            "phase": e.phase,
            "timed_out": e.timed_out,
            "trace": e.trace,
        }),
        other => json!({ "kind": "invalid_request", "message": other.to_string() }),
    };
    json!({
        "format": 1,
        "request": req.map(request_summary),
        "response": null,
        "error": error_doc,
    })
}

/// Document for an argument problem found before a request existed.
pub(crate) fn invalid_arguments(message: &str) -> Value {
    json!({
        "format": 1,
        "request": null,
        "response": null,
        "error": { "kind": "invalid_request", "message": message },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_urls_lose_their_secrets() {
        let mut req = Request::get("https://a.test/?key=s3cr3t-value");
        req.settings.secrets = vec!["s3cr3t-value".into()];
        assert_eq!(
            request_summary(&req),
            json!({ "method": "GET", "url": "https://a.test/?key=<redacted>" })
        );
    }

    #[test]
    fn invalid_request_errors() {
        let doc = failure(None, &Error::InvalidRequest("bad header".into()));
        assert_eq!(doc["format"], 1);
        assert_eq!(doc["error"]["kind"], "invalid_request");
        assert_eq!(doc["error"]["message"], "invalid request: bad header");
        assert!(doc["response"].is_null());
    }
}
