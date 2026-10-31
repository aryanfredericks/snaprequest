//! Turns a model [`Request`] into the `http::Request` hyper sends.

use http::header::{self, HeaderMap, HeaderName, HeaderValue};
use url::Url;

use super::body::{self, EncodedBody, ReqBody};
use crate::model::{Header, Request};
use crate::{Error, Result};

/// A request ready for hyper, plus what we report in the trace.
/// Can be turned into an `http::Request` more than once (for a retry).
pub(crate) struct Prepared {
    method: http::Method,
    pub uri: http::Uri,
    headers: HeaderMap,
    body: EncodedBody,
    /// Headers in the order and casing they go on the wire.
    pub sent_headers: Vec<Header>,
    pub body_len: u64,
}

impl Prepared {
    pub(crate) fn to_http(&self) -> http::Request<ReqBody> {
        let mut r = http::Request::new(self.body.to_body());
        *r.method_mut() = self.method.clone();
        *r.uri_mut() = self.uri.clone();
        *r.version_mut() = http::Version::HTTP_11;
        *r.headers_mut() = self.headers.clone();
        r
    }
}

pub(crate) async fn prepare(req: &Request, url: &Url, user_agent: &str) -> Result<Prepared> {
    let body = body::encode(&req.body).await?;
    let mut headers = HeaderMap::new();

    // Host goes first, like every mainstream client, unless the user overrides it.
    if !has_header(req, "host") {
        headers.insert(header::HOST, header_value(&host_header(url))?);
    }
    for h in req.enabled_headers() {
        let name = HeaderName::from_bytes(h.name.trim().as_bytes())
            .map_err(|_| Error::InvalidRequest(format!("invalid header name `{}`", h.name)))?;
        let value = match &body.boundary {
            // A hand-written `multipart/form-data` type without the boundary would make the
            // body unparseable, so complete it with the boundary we actually use.
            Some(b) if name == header::CONTENT_TYPE && !h.value.contains("boundary=") => {
                header_value(&format!(
                    "{}; boundary={b}",
                    h.value.trim_end_matches([';', ' '])
                ))?
            }
            _ => header_value(&h.value)?,
        };
        headers.append(name, value);
    }
    if !has_header(req, "user-agent") {
        headers.insert(header::USER_AGENT, header_value(user_agent)?);
    }
    if !has_header(req, "accept") {
        headers.insert(header::ACCEPT, HeaderValue::from_static("*/*"));
    }
    if let Some(ct) = &body.content_type {
        if !has_header(req, "content-type") {
            headers.insert(header::CONTENT_TYPE, header_value(ct)?);
        }
    }
    if !req.body.is_none() && !has_header(req, "content-length") {
        headers.insert(header::CONTENT_LENGTH, HeaderValue::from(body.len));
    }

    let sent_headers = headers
        .iter()
        .map(|(name, value)| {
            Header::new(
                title_case(name.as_str()),
                String::from_utf8_lossy(value.as_bytes()),
            )
        })
        .collect();

    let path_and_query = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    };
    let uri = path_and_query
        .parse::<http::Uri>()
        .map_err(|e| Error::InvalidRequest(format!("invalid request target: {e}")))?;

    Ok(Prepared {
        method: req.method.clone(),
        uri,
        headers,
        body_len: body.len,
        body,
        sent_headers,
    })
}

fn has_header(req: &Request, name: &str) -> bool {
    req.enabled_headers()
        .any(|h| h.name.trim().eq_ignore_ascii_case(name))
}

fn header_value(v: &str) -> Result<HeaderValue> {
    HeaderValue::from_str(v)
        .map_err(|_| Error::InvalidRequest(format!("invalid header value `{v}`")))
}

/// `Host` header value: host, plus the port when it is not the scheme's default.
fn host_header(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(port) => format!("{host}:{port}"),
        None => host.to_owned(),
    }
}

/// `content-type` → `Content-Type`, matching hyper's `title_case_headers` output.
pub(crate) fn title_case(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = true;
    for c in name.chars() {
        out.push(if upper { c.to_ascii_uppercase() } else { c });
        upper = c == '-';
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Body, KeyValue};

    fn names(p: &Prepared) -> Vec<&str> {
        p.sent_headers.iter().map(|h| h.name.as_str()).collect()
    }

    #[tokio::test]
    async fn default_headers_in_order() {
        let req = Request::get("http://example.com:8080/a?b=1").header("x-trace", "1");
        let url = req.effective_url().unwrap();
        let p = prepare(&req, &url, "snapreq/test").await.unwrap();
        assert_eq!(names(&p), ["Host", "X-Trace", "User-Agent", "Accept"]);
        assert_eq!(p.sent_headers[0].value, "example.com:8080");
        assert_eq!(p.uri, "/a?b=1");
        assert_eq!(p.to_http().headers().len(), 4);
    }

    #[tokio::test]
    async fn user_headers_override_defaults_and_disabled_are_skipped() {
        let mut req = Request::get("http://example.com/")
            .header("user-agent", "custom")
            .header("Accept", "application/json");
        req.headers.push(KeyValue::new("X-Off", "1").disabled());
        let url = req.effective_url().unwrap();
        let p = prepare(&req, &url, "snapreq/test").await.unwrap();
        assert_eq!(names(&p), ["Host", "User-Agent", "Accept"]);
        assert_eq!(p.sent_headers[1].value, "custom");
    }

    #[tokio::test]
    async fn json_body_sets_type_and_length() {
        let req = Request::new(http::Method::POST, "http://x.test/").body(Body::Json {
            content: r#"{"a":1}"#.into(),
        });
        let url = req.effective_url().unwrap();
        let p = prepare(&req, &url, "ua").await.unwrap();
        let ct = p
            .sent_headers
            .iter()
            .find(|h| h.name == "Content-Type")
            .unwrap();
        let cl = p
            .sent_headers
            .iter()
            .find(|h| h.name == "Content-Length")
            .unwrap();
        assert_eq!(ct.value, "application/json");
        assert_eq!(cl.value, "7");
        assert_eq!(p.body_len, 7);
    }

    #[tokio::test]
    async fn incomplete_multipart_content_type_gets_the_boundary() {
        let req = Request::new(http::Method::POST, "http://x.test/")
            .header("Content-Type", "multipart/form-data")
            .body(Body::Multipart { parts: vec![] });
        let url = req.effective_url().unwrap();
        let p = prepare(&req, &url, "ua").await.unwrap();
        let ct = p
            .sent_headers
            .iter()
            .find(|h| h.name == "Content-Type")
            .unwrap();
        assert!(
            ct.value
                .starts_with("multipart/form-data; boundary=----snapreq-"),
            "{}",
            ct.value
        );
        assert_eq!(
            p.sent_headers
                .iter()
                .filter(|h| h.name == "Content-Type")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn invalid_headers_are_rejected() {
        let req = Request::get("http://x.test/").header("bad header", "v");
        let url = req.effective_url().unwrap();
        let err = prepare(&req, &url, "ua").await.err().unwrap();
        assert!(err.to_string().contains("invalid header name"), "{err}");

        let req = Request::get("http://x.test/").header("x", "line\nbreak");
        let err = prepare(&req, &url, "ua").await.err().unwrap();
        assert!(err.to_string().contains("invalid header value"), "{err}");
    }

    #[test]
    fn title_case_matches_hyper() {
        assert_eq!(title_case("content-type"), "Content-Type");
        assert_eq!(title_case("x-a-b"), "X-A-B");
        assert_eq!(title_case("etag"), "Etag");
    }
}
