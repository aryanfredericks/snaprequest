//! Chaining: values captured from one response are used by the next request.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;

use bytes::Bytes;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use snapreq_collection::{Collection, Secrets};
use snapreq_core::Client;
use snapreq_test::{RunOutcome, Session};
use tokio::net::TcpListener;

/// `/login` returns a token, a user id, a cookie and a Location; `/form` returns HTML
/// with a CSRF field; `/me` echoes the headers it got.
async fn handle(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let resp = hyper::Response::builder();
    Ok(match req.uri().path() {
        "/login" => resp
            .status(201)
            .header("content-type", "application/json")
            .header("set-cookie", "sid=cookie-777; Path=/; HttpOnly")
            .header("location", "/users/7")
            .body(Full::new(Bytes::from(
                r#"{"token":"tok-secret-123","user":{"id":7,"roles":["a","b"]}}"#,
            )))
            .unwrap(),
        "/form" => resp
            .body(Full::new(Bytes::from(
                r#"<input name="csrf" value="c5rf-xyz">"#,
            )))
            .unwrap(),
        _ => {
            let mut out = format!("{}\n", req.uri());
            let mut headers: Vec<String> = req
                .headers()
                .iter()
                .filter(|(n, _)| n.as_str().starts_with("x-") || *n == "authorization")
                .map(|(n, v)| format!("{n}: {}", v.to_str().unwrap()))
                .collect();
            headers.sort();
            out.push_str(&headers.join("\n"));
            resp.body(Full::new(Bytes::from(out))).unwrap()
        }
    })
}

async fn start() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(s), service_fn(handle))
                    .await;
            });
        }
    });
    addr
}

fn collection(tag: &str, addr: SocketAddr) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snapreq-chain-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let files = [
        ("snapreq.toml", format!("format = 1\nname = \"Chain\"\n[vars]\nbase = \"http://{addr}\"\n")),
        (
            "login.toml",
            r#"method = "POST"
url = "{{base}}/login"

[capture]
token = { json = "$.access_token_missing", secret = true }
"#
            .into(),
        ),
        (
            "login-ok.toml",
            r#"method = "POST"
url = "{{base}}/login"

[capture]
token = { json = "$.token", secret = true }
user_id = "$.user.id"
roles = "$.user.roles[*]"
sid = { cookie = "sid" }
next = { header = "Location" }
code = { status = true }

[assert]
status = 201
"#
            .into(),
        ),
        (
            "form.toml",
            "url = \"{{base}}/form\"\n[capture]\ncsrf = { regex = 'name=\"csrf\" value=\"([^\"]+)\"' }\n".into(),
        ),
        (
            "me.toml",
            r#"url = "{{base}}{{next}}"

[headers]
X-User = "{{user_id}}"
X-Sid = "{{sid}}"
X-Csrf = "{{csrf}}"
X-Code = "{{code}}"

[auth]
type = "bearer"
token = "{{token}}"

[assert.body]
contains = "authorization: Bearer tok-secret-123"
"#
            .into(),
        ),
    ];
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    dir
}

#[tokio::test]
async fn captured_values_feed_later_requests() {
    let addr = start().await;
    let dir = collection("feed", addr);
    let (c, _) = Collection::load(&dir).unwrap();
    let secrets = Secrets::default();
    let mut session = Session::new(&c, None, &secrets, Client::default());

    let login = session.send("login-ok", |_| {}).await;
    assert!(login.passed(), "{login:#?}");
    let shown: Vec<(String, Option<String>)> = login
        .captures
        .iter()
        .map(|c| (c.name.clone(), c.shown().map(str::to_owned)))
        .collect();
    assert_eq!(
        shown,
        [
            ("token".into(), Some("<redacted>".into())),
            ("user_id".into(), Some("7".into())),
            ("roles".into(), Some(r#"["a","b"]"#.into())),
            ("sid".into(), Some("cookie-777".into())),
            ("next".into(), Some("/users/7".into())),
            ("code".into(), Some("201".into())),
        ]
    );
    assert!(session.send("form", |_| {}).await.passed());

    let me = session.send("me", |_| {}).await;
    assert!(me.passed(), "{me:#?}");
    let RunOutcome::Response(resp) = &me.outcome else {
        panic!()
    };
    assert_eq!(
        resp.text().unwrap(),
        "/users/7\nauthorization: Bearer tok-secret-123\nx-code: 201\nx-csrf: c5rf-xyz\nx-sid: cookie-777\nx-user: 7"
    );
    // The captured secret is hidden in the next request's trace.
    let trace = serde_json::to_string(&resp.trace).unwrap();
    assert!(!trace.contains("tok-secret-123"), "{trace}");
    assert!(
        me.request
            .unwrap()
            .secrets()
            .contains(&"tok-secret-123".to_owned())
    );
}

#[tokio::test]
async fn missing_values_fail_the_capture_and_later_placeholders() {
    let addr = start().await;
    let dir = collection("missing", addr);
    let (c, _) = Collection::load(&dir).unwrap();
    let secrets = Secrets::default();
    let mut session = Session::new(&c, None, &secrets, Client::default());

    let login = session.send("login", |_| {}).await;
    assert!(!login.passed());
    assert_eq!(
        login.captures[0].error.as_deref(),
        Some("`$.access_token_missing` matched nothing")
    );

    let me = session.send("me", |_| {}).await;
    let RunOutcome::Invalid(why) = &me.outcome else {
        panic!("{me:#?}")
    };
    assert!(why.contains("variable `next` is not defined"), "{why}");
}
