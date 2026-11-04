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

[assert.json]
"$.token" = { type = "string", starts_with = "tok-" }
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
    // Reports hide the token in this request's own checks and body as well.
    let its = [snapreq_test::Iteration {
        number: 1,
        data: None,
        result: snapreq_test::UnitRun::Requests {
            runs: vec![login],
            total: 1,
        },
    }];
    let report = snapreq_test::Report::new(
        &c,
        &snapreq_test::Unit::Requests(vec!["login-ok".into()]),
        &its,
        1,
        Vec::new(),
    );
    for text in [report.json(true).to_string(), report.html(), report.junit()] {
        assert!(!text.contains("tok-secret-123"), "{text}");
    }
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

/// Scripts (PLAN P9.3): collection, folder, request and flow-step scripts in order; pre-request
/// scripts change variables and the request, test scripts check responses and feed later requests.
#[tokio::test]
async fn scripts_run_before_and_after_requests() {
    let addr = start().await;
    let dir = std::env::temp_dir().join(format!("snapreq-scripts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("api")).unwrap();
    let files = [
        (
            "snapreq.toml",
            format!(
                r#"format = 1
name = "Scripts"
[vars]
base = "http://{addr}"
api_key = {{ secret = true }}

[script]
pre_request = '''
snapreq.variables.set("order", (snapreq.variables.get("order") || "") + "c");
'''
"#
            ),
        ),
        (
            "api/folder.toml",
            "[script]\npre_request_file = \"sign.js\"\ntest = 'snapreq.test(\"folder sees 2xx\", () => snapreq.expect(snapreq.response.status).to.be.below(300))'\n".into(),
        ),
        (
            "api/sign.js",
            r#"snapreq.variables.set("order", snapreq.variables.get("order") + "f");
snapreq.request.headers.set("X-Sig", "sig-" + snapreq.variables.get("api_key").length);
console.log("signing with", snapreq.variables.get("api_key"));
"#
            .into(),
        ),
        (
            "api/login.toml",
            r#"method = "POST"
url = "{{base}}/login"

[script]
pre_request = '''
snapreq.variables.set("order", snapreq.variables.get("order") + "r");
snapreq.request.headers.set("X-Order", "{{order}}");
'''
test = '''
const body = snapreq.response.json();
snapreq.test("token present", () => snapreq.expect(body.token).to.be.a("string"));
snapreq.test("user is 8", () => snapreq.expect(body.user.id).toBe(8));
snapreq.variables.setSecret("script_token", body.token);
snapreq.variables.set("uid", body.user.id);
'''
"#
            .into(),
        ),
        (
            "api/me.toml",
            r#"url = "{{base}}/users/{{uid}}"

[headers]
X-Token = "{{script_token}}"

[script]
test = 'console.log("me says", snapreq.response.text()); snapreq.test("ok", () => {})'
"#
            .into(),
        ),
        (
            "api/broken.toml",
            "url = \"{{base}}/x\"\n[script]\npre_request = 'snapreq.variables.get(\"api_key\").nope()'\n"
                .into(),
        ),
        (
            "check.flow.toml",
            r#"[[step]]
method = "POST"
url = "{{base}}/login"
[step.script]
test = 'snapreq.variables.set("uid", snapreq.response.json().user.id)'

[[step]]
url = "{{base}}/flow"
[step.script]
pre_request = 'snapreq.request.headers.set("X-Step", snapreq.variables.get("uid"))'
"#
            .into(),
        ),
    ];
    for (name, text) in files {
        std::fs::write(dir.join(name), text).unwrap();
    }
    let (c, _) = Collection::load(&dir).unwrap();
    let secrets = Secrets::default();
    let mut scopes_layer = snapreq_collection::Variables::default();
    scopes_layer.set_secret("api_key", "k3y-value");
    let mut session = Session::new(&c, None, &secrets, Client::default());
    session.overrides = scopes_layer;
    session.hide("k3y-value");

    let login = session.send("api/login", |_| {}).await;
    let RunOutcome::Response(_) = &login.outcome else {
        panic!("{login:#?}")
    };
    let request = login.request.as_ref().unwrap();
    let header = |name: &str| {
        request
            .headers
            .iter()
            .find(|h| h.name == name)
            .map(|h| h.value.clone())
    };
    // Collection, folder, then request; placeholders a script writes are filled in.
    assert_eq!(header("X-Order").as_deref(), Some("cfr"));
    assert_eq!(header("X-Sig").as_deref(), Some("sig-9"));
    let checks: Vec<(String, bool)> = login
        .assertions
        .iter()
        .map(|o| (format!("{} {}", o.subject, o.check), o.passed))
        .collect();
    assert_eq!(
        checks,
        [
            ("test folder sees 2xx".to_owned(), true),
            ("test token present".to_owned(), true),
            ("test user is 8".to_owned(), false),
        ]
    );
    assert_eq!(login.logs, ["signing with <redacted>"]);

    // Variables set by test scripts feed the next request; secret ones are hidden.
    let me = session.send("api/me", |_| {}).await;
    let RunOutcome::Response(resp) = &me.outcome else {
        panic!("{me:#?}")
    };
    assert!(
        resp.text().unwrap().starts_with("/users/7\n"),
        "{:?}",
        resp.text()
    );
    assert!(resp.text().unwrap().contains("x-token: tok-secret-123"));
    assert!(me.logs[1].starts_with("me says /users/7"), "{:?}", me.logs);
    assert!(!me.logs[1].contains("tok-secret-123"), "{:?}", me.logs);

    // A failing pre-request script stops the request.
    let broken = session.send("api/broken", |_| {}).await;
    let RunOutcome::Invalid(why) = &broken.outcome else {
        panic!("{broken:#?}")
    };
    assert!(
        why.starts_with("folder api pre-request script")
            || why.starts_with("request pre-request script"),
        "{why}"
    );
    assert!(
        why.contains("TypeError") && !why.contains("k3y-value"),
        "{why}"
    );

    // Flow steps add their own scripts.
    let flow = c.find_flow("check").unwrap();
    let mut session = Session::new(&c, None, &secrets, Client::default());
    let mut over = snapreq_collection::Variables::default();
    over.set_secret("api_key", "k3y-value");
    session.overrides = over;
    let run = snapreq_test::run_flow(&mut session, flow, |_| {}).await;
    let step = &run.steps[1].run;
    let RunOutcome::Response(resp) = &step.outcome else {
        panic!("{run:#?}")
    };
    assert!(
        resp.text().unwrap().contains("x-step: 7"),
        "{:?}",
        resp.text()
    );
}
