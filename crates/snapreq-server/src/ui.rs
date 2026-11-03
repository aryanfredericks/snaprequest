//! The web UI: static files built from `web/` (`npm run build` → `web/dist`), embedded into the
//! binary in release builds and read from disk in debug builds (ADR-021).
//!
//! Every `GET` that matches no API route lands here. Files are served by path; paths without a
//! file extension get `index.html` so the UI's client-side routes survive a reload. Unknown
//! `/api/…` paths stay JSON 404s. When the UI was not built, `/` explains how to build it.

use std::borrow::Cow;

use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};

use crate::api::ApiError;

#[derive(rust_embed::Embed)]
#[folder = "../../web/dist/"]
#[allow_missing = true]
struct Dist;

/// One static file.
#[derive(Debug, Clone)]
pub struct Asset {
    pub data: Cow<'static, [u8]>,
    /// Content hash, used as the ETag.
    pub hash: [u8; 32],
}

/// The built UI's file at `path` (no leading slash).
fn embedded(path: &str) -> Option<Asset> {
    Dist::get(path).map(|f| Asset {
        data: f.data,
        hash: f.metadata.sha256_hash(),
    })
}

/// Content-Security-Policy for the UI's pages: everything from the server itself, no framing.
const CSP: &str = "default-src 'self'; img-src 'self' data: blob:; style-src 'self' 'unsafe-inline'; \
                   connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'";

const NOT_BUILT: &str = "<!doctype html><meta charset=utf-8><title>SnapRequest</title>\
<h1>SnapRequest</h1><p>The server is running, but this binary was built without the web UI.</p>\
<p>Build it with <code>npm --prefix web ci &amp;&amp; npm --prefix web run build</code>, then rebuild \
<code>snapreq</code>. The API is at <a href=\"/api/v1/openapi.json\"><code>/api/v1/openapi.json</code></a>.</p>";

/// The fallback handler of the server's router.
pub async fn serve(method: Method, uri: Uri, headers: HeaderMap) -> Response {
    respond(embedded, &method, uri.path(), &headers)
}

/// Serves `path` from `assets` (separate from [`serve`] so tests can supply the files).
pub fn respond(
    assets: impl Fn(&str) -> Option<Asset>,
    method: &Method,
    path: &str,
    headers: &HeaderMap,
) -> Response {
    if path == "/api" || path.starts_with("/api/") {
        return ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("no route {path}"),
        )
        .into_response();
    }
    if method != Method::GET && method != Method::HEAD {
        return (
            StatusCode::METHOD_NOT_ALLOWED,
            [(header::ALLOW, "GET, HEAD")],
        )
            .into_response();
    }
    let file = path.trim_start_matches('/');
    let file = if file.is_empty() { "index.html" } else { file };
    let is_page = file == "index.html" || !file.rsplit('/').next().is_some_and(|n| n.contains('.'));

    let (name, asset) = match assets(file) {
        Some(asset) => (file, asset),
        // Client-side routes (`/history/42`) get the app; missing files stay 404s.
        None if is_page => match assets("index.html") {
            Some(asset) => ("index.html", asset),
            None => return not_built(),
        },
        None => return StatusCode::NOT_FOUND.into_response(),
    };

    let etag = format!("\"{}\"", hex(&asset.hash[..16]));
    let mut response = if headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| {
            v.split(',')
                .any(|tag| tag.trim() == etag || tag.trim() == "*")
        }) {
        StatusCode::NOT_MODIFIED.into_response()
    } else if method == Method::HEAD {
        StatusCode::OK.into_response()
    } else {
        asset.data.into_owned().into_response()
    };

    let mime = mime_guess::from_path(name).first_or_octet_stream();
    let h = response.headers_mut();
    if let Ok(v) = HeaderValue::from_str(mime.as_ref()) {
        h.insert(header::CONTENT_TYPE, v);
    }
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    // Vite puts content hashes into the names of everything under assets/.
    let cache = if name.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    if name.ends_with(".html") {
        h.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        );
        h.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    }
    response
}

fn not_built() -> Response {
    (
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        NOT_BUILT,
    )
        .into_response()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(path: &str) -> Option<Asset> {
        let data: &'static [u8] = match path {
            "index.html" => b"<!doctype html><div id=app></div>",
            "assets/index-abc123.js" => b"console.log(1)",
            "favicon.svg" => b"<svg/>",
            _ => return None,
        };
        Some(Asset {
            data: Cow::Borrowed(data),
            hash: [path.len() as u8; 32],
        })
    }

    fn get(path: &str) -> Response {
        respond(files, &Method::GET, path, &HeaderMap::new())
    }

    fn header(r: &Response, name: header::HeaderName) -> &str {
        r.headers()
            .get(name)
            .map_or("", |v| v.to_str().unwrap_or(""))
    }

    #[test]
    fn serves_index_and_assets_with_cache_headers() {
        let index = get("/");
        assert_eq!(index.status(), StatusCode::OK);
        assert_eq!(header(&index, header::CONTENT_TYPE), "text/html");
        assert_eq!(header(&index, header::CACHE_CONTROL), "no-cache");
        assert!(header(&index, header::CONTENT_SECURITY_POLICY).contains("frame-ancestors 'none'"));

        let js = get("/assets/index-abc123.js");
        assert_eq!(js.status(), StatusCode::OK);
        assert_eq!(header(&js, header::CONTENT_TYPE), "text/javascript");
        assert!(header(&js, header::CACHE_CONTROL).contains("immutable"));
        assert_eq!(header(&js, header::X_CONTENT_TYPE_OPTIONS), "nosniff");
        assert_eq!(header(&js, header::CONTENT_SECURITY_POLICY), "");

        assert_eq!(
            header(&get("/favicon.svg"), header::CONTENT_TYPE),
            "image/svg+xml"
        );
    }

    #[test]
    fn client_routes_get_the_app_but_missing_files_do_not() {
        let route = get("/history/42");
        assert_eq!(route.status(), StatusCode::OK);
        assert_eq!(header(&route, header::CONTENT_TYPE), "text/html");
        assert_eq!(get("/assets/missing.js").status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn unknown_api_paths_stay_json_404s() {
        let r = get("/api/v1/nope");
        assert_eq!(r.status(), StatusCode::NOT_FOUND);
        assert_eq!(header(&r, header::CONTENT_TYPE), "application/json");
    }

    #[test]
    fn etag_revalidation_and_methods() {
        let etag = header(&get("/favicon.svg"), header::ETAG).to_owned();
        let mut headers = HeaderMap::new();
        headers.insert(header::IF_NONE_MATCH, etag.parse().unwrap());
        let r = respond(files, &Method::GET, "/favicon.svg", &headers);
        assert_eq!(r.status(), StatusCode::NOT_MODIFIED);

        let r = respond(files, &Method::POST, "/", &HeaderMap::new());
        assert_eq!(r.status(), StatusCode::METHOD_NOT_ALLOWED);
        let r = respond(files, &Method::HEAD, "/", &HeaderMap::new());
        assert_eq!(r.status(), StatusCode::OK);
    }

    #[test]
    fn explains_how_to_build_a_missing_ui() {
        let r = respond(|_| None, &Method::GET, "/", &HeaderMap::new());
        assert_eq!(r.status(), StatusCode::OK);
        assert!(header(&r, header::CONTENT_TYPE).starts_with("text/html"));
    }
}
