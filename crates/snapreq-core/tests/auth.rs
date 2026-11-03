//! Authentication helpers against a local mock API.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use snapreq_core::model::{ApiKeyLocation, Auth, TraceEntry, TraceEvent};
use snapreq_core::{Client, Request};
use tokio::net::TcpListener;

/// Routes:
/// - `/basic`: 200 only for `Aladdin` / `open sesame`, otherwise 401 with a challenge
/// - `/bearer`: 200 only for token `t0ken-value`
/// - `/key-header`: 200 only with `X-API-Key: key-123456`
/// - `/key-query`: 200 only with `?api_key=key-123456`; echoes the credential back
/// - `/digest`: Digest auth (SHA-256, qop=auth) for `Mufasa` / `Circle of Life`; echoes the
///   method and body on success
/// - `/echo`: replies with the request target and headers
/// - `/to?to=<url>`: 302 to `url`
async fn handle(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let header = |name: &str| {
        req.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    };
    let query: Vec<(String, String)> =
        url::form_urlencoded::parse(req.uri().query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    let param = |name: &str| {
        query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let ok = |body: String| hyper::Response::new(Full::new(Bytes::from(body)));
    if req.uri().path() == "/digest" {
        let authorized = digest_ok(req.method().as_str(), &header("authorization"));
        let method = req.method().to_string();
        let body = req.into_body().collect().await.unwrap().to_bytes();
        return Ok(if authorized {
            ok(format!(
                "welcome {method} {}",
                String::from_utf8_lossy(&body)
            ))
        } else {
            hyper::Response::builder()
                .status(401)
                .header("www-authenticate", r#"Basic realm="other""#)
                .header(
                    "www-authenticate",
                    format!(r#"Digest realm="{REALM}", qop="auth", algorithm=SHA-256, nonce="{NONCE}", opaque="op""#),
                )
                .body(Full::new(Bytes::new()))
                .unwrap()
        });
    }
    let denied = || {
        hyper::Response::builder()
            .status(401)
            .header("www-authenticate", r#"Basic realm="test""#)
            .body(Full::new(Bytes::new()))
            .unwrap()
    };

    let resp = match req.uri().path() {
        "/basic" if header("authorization") == "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==" => {
            ok("welcome".into())
        }
        "/bearer" if header("authorization") == "Bearer t0ken-value" => ok("welcome".into()),
        "/key-header" if header("x-api-key") == "key-123456" => ok("welcome".into()),
        "/key-query" if param("api_key").as_deref() == Some("key-123456") => {
            ok(format!("you sent {}", req.uri()))
        }
        "/basic" | "/bearer" | "/key-header" | "/key-query" => denied(),
        "/echo" => {
            let mut out = format!("{}\n", req.uri());
            for (name, value) in req.headers() {
                out.push_str(&format!("{name}: {}\n", value.to_str().unwrap()));
            }
            ok(out)
        }
        "/to" => hyper::Response::builder()
            .status(302)
            .header("location", param("to").unwrap())
            .body(Full::new(Bytes::new()))
            .unwrap(),
        _ => hyper::Response::builder()
            .status(404)
            .body(Full::new(Bytes::new()))
            .unwrap(),
    };
    Ok(resp)
}

const REALM: &str = "test@snapreq";
const NONCE: &str = "dcd98b7102dd2f0e8b11d0f600bfb0c093";

fn sha256(s: &str) -> String {
    ring::digest::digest(&ring::digest::SHA256, s.as_bytes())
        .as_ref()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Checks a Digest `Authorization` header the way a server does.
fn digest_ok(method: &str, header: &str) -> bool {
    let Some(rest) = header.strip_prefix("Digest ") else {
        return false;
    };
    let params: std::collections::HashMap<&str, &str> = rest
        .split(", ")
        .filter_map(|p| p.split_once('='))
        .map(|(k, v)| (k, v.trim_matches('"')))
        .collect();
    let get = |k: &str| params.get(k).copied().unwrap_or_default();
    if get("username") != "Mufasa" || get("nonce") != NONCE || get("opaque") != "op" {
        return false;
    }
    let ha1 = sha256(&format!("Mufasa:{REALM}:Circle of Life"));
    let ha2 = sha256(&format!("{method}:{}", get("uri")));
    let expected = sha256(&format!(
        "{ha1}:{NONCE}:{}:{}:{}:{ha2}",
        get("nc"),
        get("cnonce"),
        get("qop")
    ));
    get("response") == expected
}

async fn start_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service_fn(handle))
                    .await;
            });
        }
    });
    addr
}

fn url(addr: SocketAddr, path: &str) -> String {
    format!("http://{addr}{path}")
}

fn redirect(from: SocketAddr, to: &str) -> String {
    let to: String = url::form_urlencoded::byte_serialize(to.as_bytes()).collect();
    url(from, &format!("/to?to={to}"))
}

/// Every piece of text in the trace, serialized.
fn trace_text(trace: &[TraceEntry]) -> String {
    serde_json::to_string(trace).unwrap()
}

fn infos(trace: &[TraceEntry]) -> Vec<&str> {
    trace
        .iter()
        .filter_map(|e| match &e.event {
            TraceEvent::Info { message } => Some(message.as_str()),
            _ => None,
        })
        .collect()
}

fn basic() -> Auth {
    Auth::Basic {
        username: "Aladdin".into(),
        password: "open sesame".into(),
    }
}

fn key(location: ApiKeyLocation) -> Auth {
    Auth::ApiKey {
        name: match location {
            ApiKeyLocation::Header => "X-API-Key",
            ApiKeyLocation::Query => "api_key",
        }
        .into(),
        value: "key-123456".into(),
        location,
    }
}

#[tokio::test]
async fn each_auth_type_is_accepted_by_the_mock() {
    let addr = start_server().await;
    let client = Client::default();
    for (path, auth) in [
        ("/basic", basic()),
        (
            "/bearer",
            Auth::Bearer {
                token: "t0ken-value".into(),
            },
        ),
        ("/key-header", key(ApiKeyLocation::Header)),
        ("/key-query", key(ApiKeyLocation::Query)),
    ] {
        let without = client
            .execute(&Request::get(url(addr, path)))
            .await
            .unwrap();
        assert_eq!(without.status, 401, "{path} without auth");

        let resp = client
            .execute(&Request::get(url(addr, path)).auth(auth.clone()))
            .await
            .unwrap();
        assert_eq!(resp.status, 200, "{path} with {auth:?}");
    }
}

#[tokio::test]
async fn credentials_are_hidden_in_the_trace() {
    let addr = start_server().await;
    let client = Client::default();

    let resp = client
        .execute(&Request::get(url(addr, "/basic")).auth(basic()))
        .await
        .unwrap();
    let text = trace_text(&resp.trace);
    assert!(text.contains("Basic <redacted>"), "{text}");
    assert!(!text.contains("QWxhZGRpbjpvcGVuIHNlc2FtZQ=="), "{text}");

    // A key in a custom header or the query string is hidden too, including the URL
    // in the trace and the server echoing it back.
    let mut req = Request::get(url(addr, "/key-query")).auth(key(ApiKeyLocation::Query));
    req.settings.capture_wire = true;
    let resp = client.execute(&req).await.unwrap();
    assert!(resp.text().unwrap().contains("key-123456"));
    assert!(resp.url.contains("api_key=key-123456"));
    let text = trace_text(&resp.trace);
    assert!(!text.contains("key-123456"), "{text}");
    assert!(text.contains("api_key=<redacted>"), "{text}");

    let resp = client
        .execute(&Request::get(url(addr, "/key-header")).auth(key(ApiKeyLocation::Header)))
        .await
        .unwrap();
    assert!(!trace_text(&resp.trace).contains("key-123456"));
}

#[tokio::test]
async fn auth_follows_same_origin_redirects_only() {
    let a = start_server().await;
    let b = start_server().await;
    let client = Client::default();

    let same = Request::get(redirect(a, "/basic")).auth(basic());
    assert_eq!(client.execute(&same).await.unwrap().status, 200);
    let same = Request::get(redirect(a, "/key-query")).auth(key(ApiKeyLocation::Query));
    let resp = client.execute(&same).await.unwrap();
    assert_eq!(resp.status, 200);
    // The key is not repeated on the redirected URL.
    assert_eq!(resp.url.matches("api_key=").count(), 1, "{}", resp.url);

    for auth in [
        basic(),
        key(ApiKeyLocation::Query),
        key(ApiKeyLocation::Header),
    ] {
        let kind = auth.kind();
        let req = Request::get(redirect(a, &url(b, "/echo"))).auth(auth);
        let resp = client.execute(&req).await.unwrap();
        let echo = resp.text().unwrap();
        assert!(!echo.contains("authorization"), "{echo}");
        assert!(!echo.contains("x-api-key"), "{echo}");
        assert!(!echo.contains("api_key"), "{echo}");
        assert!(
            infos(&resp.trace)
                .contains(&format!("cross-origin redirect: not sending {kind} auth").as_str()),
            "{:?}",
            infos(&resp.trace)
        );
    }
}

#[tokio::test]
async fn a_hand_written_header_wins_over_auth() {
    let addr = start_server().await;
    let req = Request::get(url(addr, "/echo"))
        .header("Authorization", "Custom abc")
        .auth(Auth::Bearer {
            token: "t0ken-value".into(),
        });
    let resp = Client::default().execute(&req).await.unwrap();
    let echo = resp.text().unwrap();
    assert!(echo.contains("authorization: Custom abc\n"), "{echo}");
    assert!(!echo.contains("Bearer"), "{echo}");
    assert!(
        infos(&resp.trace)
            .contains(&"bearer auth not applied: the request sets the Authorization header itself"),
        "{:?}",
        infos(&resp.trace)
    );
}

#[tokio::test]
async fn invalid_credentials_fail_without_showing_them() {
    let err = Client::default()
        .execute(&Request::get("http://127.0.0.1:1/").auth(Auth::Bearer {
            token: "line\nbreak-secret".into(),
        }))
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("bearer auth"), "{err}");
    assert!(!err.contains("break-secret"), "{err}");
}

fn digest(password: &str) -> Auth {
    Auth::Digest {
        username: "Mufasa".into(),
        password: password.into(),
    }
}

fn events(trace: &[TraceEntry]) -> Vec<String> {
    trace
        .iter()
        .filter_map(|e| match &e.event {
            TraceEvent::ResponseHeaders { status, .. } => Some(status.to_string()),
            TraceEvent::AuthChallenge { status, scheme } => {
                Some(format!("challenge {status} {scheme}"))
            }
            TraceEvent::Redirect { status, .. } => Some(format!("redirect {status}")),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn digest_answers_the_challenge_and_resends_the_body() {
    let addr = start_server().await;
    let client = Client::default();

    let req = Request::new(http::Method::PUT, url(addr, "/digest?x=1"))
        .body(snapreq_core::Body::Text {
            content: "payload".into(),
            content_type: None,
        })
        .auth(digest("Circle of Life"));
    let resp = client.execute(&req).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(resp.text(), Some("welcome PUT payload"));
    assert_eq!(events(&resp.trace), ["401", "challenge 401 digest", "200"]);
    // Timings describe the authenticated exchange: it reused the connection.
    assert!(resp.timings.connect.is_none());
    assert!(trace_text(&resp.trace).contains("Digest <redacted>"));
    assert!(!trace_text(&resp.trace).contains("Circle of Life"));

    // A wrong password is tried once; the final 401 is returned.
    let resp = client
        .execute(&Request::get(url(addr, "/digest")).auth(digest("wrong")))
        .await
        .unwrap();
    assert_eq!(resp.status, 401);
    assert_eq!(events(&resp.trace), ["401", "challenge 401 digest", "401"]);
}

#[tokio::test]
async fn digest_is_answered_again_after_a_redirect() {
    let addr = start_server().await;
    let resp = Client::default()
        .execute(&Request::get(redirect(addr, "/digest")).auth(digest("Circle of Life")))
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(
        events(&resp.trace),
        ["302", "redirect 302", "401", "challenge 401 digest", "200"]
    );
}

#[tokio::test]
async fn digest_without_a_challenge_is_a_normal_request() {
    let addr = start_server().await;
    let client = Client::default();
    let resp = client
        .execute(&Request::get(url(addr, "/echo")).auth(digest("x")))
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    assert!(!resp.text().unwrap().contains("authorization"));

    // A 401 with only a Basic challenge is returned as is.
    let resp = client
        .execute(&Request::get(url(addr, "/basic")).auth(digest("x")))
        .await
        .unwrap();
    assert_eq!(resp.status, 401);
    assert_eq!(events(&resp.trace), ["401"]);
}

#[tokio::test]
async fn trace_events_stream_live_and_redacted() {
    let addr = start_server().await;
    let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let sink_seen = seen.clone();
    let sink: snapreq_core::TraceSink = std::sync::Arc::new(move |e: &TraceEntry| {
        sink_seen.lock().unwrap().push(e.clone());
    });
    let resp = Client::default()
        .execute_with(&Request::get(url(addr, "/basic")).auth(basic()), Some(sink))
        .await
        .unwrap();
    let live = seen.lock().unwrap().clone();
    assert_eq!(live, resp.trace, "live events match the final trace");
    assert!(!trace_text(&live).contains("QWxhZGRpbjpvcGVuIHNlc2FtZQ=="));
}
