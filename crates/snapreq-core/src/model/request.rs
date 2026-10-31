//! The request a user wants to send.

use http::Method;
use serde::{Deserialize, Serialize};
use url::Url;

use super::{Body, KeyValue, RequestSettings};
use crate::{Error, Result};

/// A fully resolved request (variables already interpolated) ready to be executed.
///
/// Headers and query parameters are ordered lists rather than maps: order is preserved,
/// duplicates are allowed, and individual entries can be disabled without deleting them,
/// which is how people work in an API client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    /// HTTP method. Any valid token is accepted, including custom ones such as `PURGE`.
    #[serde(with = "crate::serde_util::method")]
    pub method: Method,
    /// Target URL. May already contain a query string; `query` entries are appended to it.
    pub url: String,
    /// Extra query parameters, appended in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub query: Vec<KeyValue>,
    /// Request headers, sent in order.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    /// Request body.
    #[serde(default, skip_serializing_if = "Body::is_none")]
    pub body: Body,
    /// Per-request execution settings.
    #[serde(default)]
    pub settings: RequestSettings,
}

impl Request {
    /// Creates a request with no headers, no body and default settings.
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        Self {
            method,
            url: url.into(),
            query: Vec::new(),
            headers: Vec::new(),
            body: Body::None,
            settings: RequestSettings::default(),
        }
    }

    /// Shorthand for a `GET` request.
    pub fn get(url: impl Into<String>) -> Self {
        Self::new(Method::GET, url)
    }

    /// Adds an enabled header.
    #[must_use]
    pub fn header(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.headers.push(KeyValue::new(name, value));
        self
    }

    /// Adds an enabled query parameter.
    #[must_use]
    pub fn query(mut self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.query.push(KeyValue::new(name, value));
        self
    }

    /// Sets the body.
    #[must_use]
    pub fn body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    /// Enabled headers, in order.
    pub fn enabled_headers(&self) -> impl Iterator<Item = &KeyValue> {
        self.headers.iter().filter(|h| h.enabled)
    }

    /// Parses `url` and appends the enabled `query` parameters.
    ///
    /// Only `http` and `https` URLs with a host are accepted.
    pub fn effective_url(&self) -> Result<Url> {
        let invalid = |reason: String| Error::InvalidUrl {
            url: self.url.clone(),
            reason,
        };

        let mut url = Url::parse(self.url.trim()).map_err(|e| invalid(e.to_string()))?;
        if !matches!(url.scheme(), "http" | "https") {
            return Err(invalid(format!(
                "unsupported scheme `{}` (expected http or https)",
                url.scheme()
            )));
        }
        if url.host_str().is_none_or(str::is_empty) {
            return Err(invalid("missing host".into()));
        }

        let mut enabled = self.query.iter().filter(|q| q.enabled).peekable();
        if enabled.peek().is_some() {
            let mut pairs = url.query_pairs_mut();
            for q in enabled {
                pairs.append_pair(&q.name, &q.value);
            }
        }
        Ok(url)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_url_appends_enabled_query_params_in_order() {
        let mut req = Request::get("https://example.com/search?q=rust")
            .query("page", "2")
            .query("tag", "a b");
        req.query.push(KeyValue::new("skip", "me").disabled());
        req.query.push(KeyValue::new("tag", "c"));

        let url = req.effective_url().unwrap();
        assert_eq!(
            url.as_str(),
            "https://example.com/search?q=rust&page=2&tag=a+b&tag=c"
        );
    }

    #[test]
    fn effective_url_without_params_is_unchanged() {
        let url = Request::get("http://localhost:8080/a/b")
            .effective_url()
            .unwrap();
        assert_eq!(url.as_str(), "http://localhost:8080/a/b");
    }

    #[test]
    fn effective_url_rejects_bad_input() {
        for (input, needle) in [
            ("not a url", "relative URL"),
            ("ftp://example.com", "unsupported scheme `ftp`"),
            ("file:///etc/passwd", "unsupported scheme `file`"),
        ] {
            let err = Request::get(input).effective_url().unwrap_err().to_string();
            assert!(err.contains(needle), "`{input}` gave `{err}`");
        }
    }

    #[test]
    fn custom_methods_round_trip() {
        let req = Request::new(Method::from_bytes(b"PURGE").unwrap(), "http://x.test");
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains(r#""method":"PURGE""#));
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);
    }

    #[test]
    fn invalid_method_is_rejected() {
        let err = serde_json::from_str::<Request>(r#"{"method":"GE T","url":"http://x"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid HTTP method"), "{err}");
    }

    #[test]
    fn minimal_json_uses_defaults() {
        let req: Request =
            serde_json::from_str(r#"{"method":"GET","url":"http://x.test"}"#).unwrap();
        assert_eq!(req, Request::get("http://x.test"));
    }
}
