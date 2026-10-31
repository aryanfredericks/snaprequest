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
    /// Origin-form target (`/path?query`) for HTTP/1.1.
    pub uri: http::Uri,
    /// Absolute URI for HTTP/2, which carries scheme and authority as pseudo-headers.
    absolute_uri: http::Uri,
    headers: HeaderMap,
    body: EncodedBody,
    /// Headers in the order and casing they go on the wire.
    pub sent_headers: Vec<Header>,
    pub body_len: u64,
}

/// Headers that only make sense for one HTTP/1.1 connection; HTTP/2 forbids them.
const CONNECTION_SPECIFIC: [&str; 5] = [
    "connection",
    "keep-alive",
    "proxy-connection",
    "transfer-encoding",
    "upgrade",
];

impl Prepared {
    pub(crate) fn method(&self) -> &http::Method {
        &self.method
    }

    /// A fresh body stream.
    pub(crate) fn body_stream(&self) -> ReqBody {
        self.body.to_body()
    }

    /// Builds the request for HTTP/1.1 or HTTP/2.
    pub(crate) fn to_http(&self, h2: bool) -> http::Request<ReqBody> {
        self.to_http_with_body(h2, self.body.to_body())
    }

    pub(crate) fn to_http_with_body(&self, h2: bool, body: ReqBody) -> http::Request<ReqBody> {
        let mut r = http::Request::new(body);
        *r.method_mut() = self.method.clone();
        if h2 {
            *r.uri_mut() = self.absolute_uri.clone();
            *r.version_mut() = http::Version::HTTP_2;
            *r.headers_mut() = self.h2_headers();
        } else {
            *r.uri_mut() = self.uri.clone();
            *r.version_mut() = http::Version::HTTP_11;
            *r.headers_mut() = self.headers.clone();
        }
        r
    }

    /// Headers as they go out over HTTP/2: `Host` becomes the `:authority`
    /// pseudo-header and connection-specific headers are removed.
    fn h2_headers(&self) -> HeaderMap {
        // Rebuilt rather than `remove`d: removal swaps entries and would reorder headers.
        let mut headers = HeaderMap::with_capacity(self.headers.len());
        for (name, value) in &self.headers {
            let forbidden = *name == header::HOST
                || CONNECTION_SPECIFIC.contains(&name.as_str())
                || (*name == header::TE && !value.as_bytes().eq_ignore_ascii_case(b"trailers"));
            if !forbidden {
                headers.append(name.clone(), value.clone());
            }
        }
        headers
    }

    /// HTTP/1.1 request in absolute form for an HTTP proxy, with `Proxy-Authorization`
    /// if the proxy has credentials.
    pub(crate) fn to_http_via_proxy(
        &self,
        authorization: Option<&str>,
    ) -> Result<http::Request<ReqBody>> {
        let mut r = self.to_http(false);
        *r.uri_mut() = self.absolute_uri.clone();
        if let Some(auth) = authorization {
            r.headers_mut()
                .append(header::PROXY_AUTHORIZATION, header_value(auth)?);
        }
        Ok(r)
    }

    /// Headers for the trace, in the casing used on the wire.
    pub(crate) fn trace_headers(&self, h2: bool) -> Vec<Header> {
        if !h2 {
            return self.sent_headers.clone();
        }
        self.h2_headers()
            .iter()
            .map(|(n, v)| Header::new(n.as_str(), String::from_utf8_lossy(v.as_bytes())))
            .collect()
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
    if req.settings.decompress && !has_header(req, "accept-encoding") {
        headers.insert(
            header::ACCEPT_ENCODING,
            HeaderValue::from_static(super::decode::ACCEPT_ENCODING),
        );
    }
    if let Some(ct) = &body.content_type
        && !has_header(req, "content-type")
    {
        headers.insert(header::CONTENT_TYPE, header_value(ct)?);
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
    let mut without_fragment = url.clone();
    without_fragment.set_fragment(None);
    let absolute_uri = without_fragment
        .as_str()
        .parse::<http::Uri>()
        .map_err(|e| Error::InvalidRequest(format!("invalid request URL: {e}")))?;

    Ok(Prepared {
        method: req.method.clone(),
        uri,
        absolute_uri,
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
        assert_eq!(
            names(&p),
            ["Host", "X-Trace", "User-Agent", "Accept", "Accept-Encoding"]
        );
        assert_eq!(p.sent_headers[0].value, "example.com:8080");
        assert_eq!(p.uri, "/a?b=1");
        assert_eq!(p.to_http(false).headers().len(), 5);
        let h2 = p.to_http(true);
        assert_eq!(h2.uri(), "http://example.com:8080/a?b=1");
        assert!(h2.headers().get("host").is_none());
        assert_eq!(p.trace_headers(true)[0].name, "x-trace");
    }

    #[tokio::test]
    async fn user_headers_override_defaults_and_disabled_are_skipped() {
        let mut req = Request::get("http://example.com/")
            .header("user-agent", "custom")
            .header("Accept", "application/json");
        req.headers.push(KeyValue::new("X-Off", "1").disabled());
        let url = req.effective_url().unwrap();
        let p = prepare(&req, &url, "snapreq/test").await.unwrap();
        assert_eq!(
            names(&p),
            ["Host", "User-Agent", "Accept", "Accept-Encoding"]
        );
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
    async fn connection_specific_headers_are_dropped_for_http2() {
        let req = Request::get("https://x.test/#frag")
            .header("Connection", "keep-alive")
            .header("TE", "gzip")
            .header("X-Ok", "1");
        let url = req.effective_url().unwrap();
        let p = prepare(&req, &url, "ua").await.unwrap();
        let h2 = p.to_http(true);
        assert_eq!(h2.uri(), "https://x.test/");
        assert!(h2.headers().get("connection").is_none());
        assert!(h2.headers().get("te").is_none());
        assert_eq!(h2.headers()["x-ok"], "1");
        // HTTP/1.1 keeps what the user asked for.
        assert_eq!(p.to_http(false).headers()["connection"], "keep-alive");
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
