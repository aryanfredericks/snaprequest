//! Deciding whether and how to follow a redirect.

use http::Method;
use url::Url;

use crate::model::{Body, Request, Response};

/// Headers that carry credentials and must not leak to another origin.
const CREDENTIAL_HEADERS: [&str; 3] = ["authorization", "cookie", "proxy-authorization"];

/// The next hop of a redirect chain.
#[derive(Debug)]
pub(crate) struct NextHop {
    pub request: Request,
    pub url: Url,
    /// Human-readable notes for the trace (method change, dropped headers, downgrade).
    pub notes: Vec<String>,
}

/// Why a redirect response cannot be followed.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum RedirectError {
    InvalidLocation(String),
    UnsupportedScheme(String),
}

impl std::fmt::Display for RedirectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidLocation(why) => write!(f, "invalid Location header: {why}"),
            Self::UnsupportedScheme(s) => write!(f, "cannot follow redirect to a `{s}` URL"),
        }
    }
}

/// Returns the next hop if `resp` is a followable redirect, `Ok(None)` if it is not
/// a redirect (or has no `Location`).
///
/// Rules, matching browsers and mainstream API clients:
/// - 303 turns any method except HEAD into GET without a body.
/// - 301 and 302 turn POST into GET without a body.
/// - 307 and 308 keep the method and body.
/// - On a change of origin, `Authorization`, `Cookie`, `Proxy-Authorization` and an explicit
///   `Host` header are dropped.
pub(crate) fn next_hop(
    current: &Request,
    url: &Url,
    resp: &Response,
) -> Result<Option<NextHop>, RedirectError> {
    if !matches!(resp.status, 301 | 302 | 303 | 307 | 308) {
        return Ok(None);
    }
    let Some(location) = resp.header("location") else {
        return Ok(None);
    };
    let next_url = url
        .join(location.trim())
        .map_err(|e| RedirectError::InvalidLocation(format!("`{location}`: {e}")))?;
    if !matches!(next_url.scheme(), "http" | "https") {
        return Err(RedirectError::UnsupportedScheme(
            next_url.scheme().to_owned(),
        ));
    }

    let mut next = current.clone();
    next.url = next_url.to_string();
    // The Location already contains the query string.
    next.query.clear();
    let mut notes = Vec::new();

    let to_get = match resp.status {
        303 => current.method != Method::HEAD,
        301 | 302 => current.method == Method::POST,
        _ => false,
    };
    if to_get && (current.method != Method::GET || !current.body.is_none()) {
        notes.push(format!(
            "{} redirect: switching {} to GET and dropping the body",
            resp.status, current.method
        ));
        next.method = Method::GET;
        next.body = Body::None;
        next.headers.retain(|h| {
            !h.name.eq_ignore_ascii_case("content-type")
                && !h.name.eq_ignore_ascii_case("content-length")
        });
    }

    if url.origin() != next_url.origin() {
        let before = next.headers.len();
        let mut dropped: Vec<String> = next
            .headers
            .iter()
            .filter(|h| is_origin_bound(&h.name))
            .map(|h| h.name.clone())
            .collect();
        dropped.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        next.headers.retain(|h| !is_origin_bound(&h.name));
        if next.headers.len() != before {
            notes.push(format!(
                "cross-origin redirect: dropped {} header(s)",
                dropped.join(", ")
            ));
        }
    }

    if url.scheme() == "https" && next_url.scheme() == "http" {
        notes.push("redirect downgrades from HTTPS to plain HTTP".into());
    }

    Ok(Some(NextHop {
        request: next,
        url: next_url,
        notes,
    }))
}

fn is_origin_bound(name: &str) -> bool {
    let name = name.trim();
    name.eq_ignore_ascii_case("host")
        || CREDENTIAL_HEADERS
            .iter()
            .any(|c| name.eq_ignore_ascii_case(c))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Header, HttpVersion, Sizes, Timings};

    fn redirect(status: u16, location: &str) -> Response {
        Response {
            status,
            version: HttpVersion::Http11,
            url: String::new(),
            headers: vec![Header::new("location", location)],
            body: bytes::Bytes::new(),
            remote_addr: None,
            timings: Timings::default(),
            sizes: Sizes::default(),
            trace: vec![],
        }
    }

    fn post() -> Request {
        Request::new(Method::POST, "https://api.test/a?x=1")
            .header("Authorization", "Bearer secret")
            .header("Content-Type", "application/json")
            .header("X-Keep", "1")
            .query("y", "2")
            .body(Body::Json {
                content: "{}".into(),
            })
    }

    fn hop(req: &Request, resp: &Response) -> NextHop {
        let url = req.effective_url().unwrap();
        next_hop(req, &url, resp).unwrap().unwrap()
    }

    fn header_names(r: &Request) -> Vec<&str> {
        r.headers.iter().map(|h| h.name.as_str()).collect()
    }

    #[test]
    fn relative_location_is_resolved_and_query_params_are_not_repeated() {
        let next = hop(
            &Request::get("https://api.test/a/b?x=1").query("y", "2"),
            &redirect(302, "../c?z=3"),
        );
        assert_eq!(next.url.as_str(), "https://api.test/c?z=3");
        assert!(next.request.query.is_empty());
        assert_eq!(next.request.effective_url().unwrap(), next.url);
    }

    #[test]
    fn see_other_turns_post_into_get() {
        let next = hop(&post(), &redirect(303, "/done"));
        assert_eq!(next.request.method, Method::GET);
        assert!(next.request.body.is_none());
        assert_eq!(header_names(&next.request), ["Authorization", "X-Keep"]);
        assert_eq!(
            next.notes,
            ["303 redirect: switching POST to GET and dropping the body"]
        );
    }

    #[test]
    fn temporary_and_permanent_redirects_keep_method_and_body() {
        for status in [307, 308] {
            let next = hop(&post(), &redirect(status, "/again"));
            assert_eq!(next.request.method, Method::POST);
            assert!(!next.request.body.is_none());
            assert!(next.notes.is_empty());
        }
    }

    #[test]
    fn found_turns_only_post_into_get() {
        assert_eq!(
            hop(&post(), &redirect(302, "/x")).request.method,
            Method::GET
        );
        let put = Request::new(Method::PUT, "https://api.test/");
        assert_eq!(hop(&put, &redirect(302, "/x")).request.method, Method::PUT);
        let head = Request::new(Method::HEAD, "https://api.test/");
        assert_eq!(
            hop(&head, &redirect(303, "/x")).request.method,
            Method::HEAD
        );
    }

    #[test]
    fn cross_origin_drops_credentials() {
        let req = post().header("Cookie", "a=1").header("host", "api.test");
        let next = hop(&req, &redirect(307, "https://other.test/"));
        assert_eq!(header_names(&next.request), ["Content-Type", "X-Keep"]);
        assert_eq!(
            next.notes,
            ["cross-origin redirect: dropped Authorization, Cookie, host header(s)"]
        );

        // Same origin keeps them.
        let same = hop(&req, &redirect(307, "https://api.test/other"));
        assert!(header_names(&same.request).contains(&"Authorization"));
    }

    #[test]
    fn https_to_http_is_flagged() {
        let next = hop(
            &Request::get("https://api.test/"),
            &redirect(301, "http://api.test/"),
        );
        assert!(next.notes.iter().any(|n| n.contains("downgrades")));
    }

    #[test]
    fn non_redirects_and_bad_locations() {
        let req = Request::get("https://api.test/");
        let url = req.effective_url().unwrap();
        assert!(
            next_hop(&req, &url, &redirect(200, "/x"))
                .unwrap()
                .is_none()
        );
        assert!(
            next_hop(&req, &url, &redirect(304, "/x"))
                .unwrap()
                .is_none()
        );
        let mut no_location = redirect(302, "/x");
        no_location.headers.clear();
        assert!(next_hop(&req, &url, &no_location).unwrap().is_none());

        assert_eq!(
            next_hop(&req, &url, &redirect(302, "ftp://files.test/x")).unwrap_err(),
            RedirectError::UnsupportedScheme("ftp".into())
        );
        assert!(matches!(
            next_hop(&req, &url, &redirect(302, "http://[bad")).unwrap_err(),
            RedirectError::InvalidLocation(_)
        ));
    }
}
