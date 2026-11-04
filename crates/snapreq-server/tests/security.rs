//! Hardening end to end (PLAN P8.5): security headers, rate limits, the Host check and the audit log.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request, Response};
use snapreq_server::config::{AuthConfig, AuthMode};
use snapreq_server::store::{Retention, Store};
use snapreq_server::{Config, Server};

struct Srv {
    base: String,
    store: Store,
}

async fn start(tag: &str, mode: AuthMode) -> Srv {
    let root = std::env::temp_dir().join(format!("snapreq-sec-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: root.join("data"),
        workspace: root.join("workspace"),
        allow_remote: false,
        retention: Retention::default(),
        outbound: Default::default(),
        auth: AuthConfig {
            mode,
            allowed_hosts: vec!["devbox".into()],
            ..AuthConfig::default()
        },
        oidc: None,
    };
    let server = Server::bind(config).await.unwrap();
    let base = format!("http://{}", server.local_addr().unwrap());
    let store = server.store().clone();
    tokio::spawn(server.run(std::future::pending()));
    Srv { base, store }
}

async fn send(method: &str, url: &str, headers: &[(&str, &str)], body: Option<Value>) -> Response {
    let mut req = Request::new(http::Method::from_bytes(method.as_bytes()).unwrap(), url);
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    if let Some(b) = body {
        req.body = Body::Json {
            content: b.to_string(),
        };
    }
    Client::default().execute(&req).await.unwrap()
}

fn json_of(r: &Response) -> Value {
    serde_json::from_slice(&r.body).unwrap_or(Value::Null)
}

#[tokio::test]
async fn every_response_carries_security_headers() {
    let srv = start("headers", AuthMode::None).await;
    for path in ["/api/v1/version", "/", "/healthz", "/api/v1/nope"] {
        let r = send("GET", &format!("{}{path}", srv.base), &[], None).await;
        assert_eq!(
            r.header("x-content-type-options"),
            Some("nosniff"),
            "{path}"
        );
        assert_eq!(r.header("x-frame-options"), Some("DENY"), "{path}");
        assert_eq!(r.header("referrer-policy"), Some("no-referrer"), "{path}");
        assert_eq!(
            r.header("cross-origin-opener-policy"),
            Some("same-origin"),
            "{path}"
        );
        assert!(
            r.header("strict-transport-security").is_none(),
            "only with secure cookies"
        );
    }
    let api = send("GET", &format!("{}/api/v1/version", srv.base), &[], None).await;
    assert_eq!(api.header("cache-control"), Some("no-store"));
    assert!(
        api.header("content-security-policy")
            .unwrap()
            .contains("default-src 'none'")
    );
    // The UI keeps its own policy.
    let page = send("GET", &format!("{}/", srv.base), &[], None).await;
    assert!(
        page.header("content-security-policy")
            .unwrap()
            .contains("default-src 'self'")
    );
}

#[tokio::test]
async fn a_server_without_login_refuses_rebound_host_names() {
    let srv = start("host", AuthMode::None).await;
    let url = format!("{}/api/v1/collections", srv.base);
    let evil = send("GET", &url, &[("Host", "evil.example:7878")], None).await;
    assert_eq!(evil.status, 403);
    assert!(
        json_of(&evil)["error"]["message"]
            .as_str()
            .unwrap()
            .contains("allowed_hosts")
    );
    for ok in ["localhost:7878", "127.0.0.1", "devbox:7878"] {
        assert_eq!(
            send("GET", &url, &[("Host", ok)], None).await.status,
            200,
            "{ok}"
        );
    }
    // The UI and health check are not data; with accounts, cookies protect the API instead.
    assert_eq!(
        send(
            "GET",
            &format!("{}/healthz", srv.base),
            &[("Host", "evil.example")],
            None
        )
        .await
        .status,
        200
    );
    let accounts = start("host-accounts", AuthMode::Accounts).await;
    let r = send(
        "GET",
        &format!("{}/api/v1/auth/status", accounts.base),
        &[("Host", "evil.example")],
        None,
    )
    .await;
    assert_eq!(r.status, 200);
}

#[tokio::test]
async fn failed_logins_are_limited() {
    let srv = start("login", AuthMode::Accounts).await;
    srv.store.create_user("ana", "ana-password").await.unwrap();
    let login = |password: &'static str| {
        let url = format!("{}/api/v1/auth/login", srv.base);
        async move {
            send(
                "POST",
                &url,
                &[("X-SnapReq-CSRF", "1")],
                Some(json!({ "username": "Ana", "password": password })),
            )
            .await
        }
    };
    assert_eq!(login("ana-password").await.status, 200);
    for _ in 0..10 {
        assert_eq!(login("guess").await.status, 401);
    }
    let blocked = login("ana-password").await;
    assert_eq!(blocked.status, 429, "even the right password waits");
    assert_eq!(json_of(&blocked)["error"]["kind"], "rate_limited");
    assert!(
        blocked
            .header("retry-after")
            .unwrap()
            .parse::<u64>()
            .unwrap()
            > 0
    );
    // Other users are not affected by Ana's lockout.
    srv.store.create_user("bo", "bo-password").await.unwrap();
    let url = format!("{}/api/v1/auth/login", srv.base);
    let bo = send(
        "POST",
        &url,
        &[("X-SnapReq-CSRF", "1")],
        Some(json!({ "username": "bo", "password": "bo-password" })),
    )
    .await;
    assert_eq!(bo.status, 200);
}

#[tokio::test]
async fn unknown_tokens_are_limited() {
    let srv = start("tokens", AuthMode::Accounts).await;
    let url = format!("{}/api/v1/history", srv.base);
    for _ in 0..50 {
        let r = send(
            "GET",
            &url,
            &[("Authorization", "Bearer snapreq_pat_guess")],
            None,
        )
        .await;
        assert_eq!(r.status, 401);
    }
    let r = send(
        "GET",
        &url,
        &[("Authorization", "Bearer snapreq_pat_guess")],
        None,
    )
    .await;
    assert_eq!(r.status, 429);
}

#[tokio::test]
async fn the_audit_log_shows_users_their_own_events_and_owners_their_workspaces() {
    let srv = start("audit", AuthMode::Accounts).await;
    let ana = srv.store.create_user("ana", "ana-password").await.unwrap();
    let bo = srv.store.create_user("bo", "bo-password").await.unwrap();
    let (_, ana_token) = srv.store.create_token(ana.id, "t", None).await.unwrap();
    let (_, bo_token) = srv.store.create_token(bo.id, "t", None).await.unwrap();
    let auth = |t: &str| format!("Bearer {t}");
    let base = &srv.base;
    let a = auth(&ana_token);
    let b = auth(&bo_token);

    // A failed and a successful login, then workspace changes.
    let url = format!("{base}/api/v1/auth/login");
    send(
        "POST",
        &url,
        &[("X-SnapReq-CSRF", "1")],
        Some(json!({ "username": "bo", "password": "nope-nope" })),
    )
    .await;
    send(
        "POST",
        &url,
        &[("X-SnapReq-CSRF", "1")],
        Some(json!({ "username": "bo", "password": "bo-password" })),
    )
    .await;
    send(
        "POST",
        &format!("{base}/api/v1/workspaces"),
        &[("Authorization", &b)],
        Some(json!({ "id": "bo-ws" })),
    )
    .await;
    send(
        "PUT",
        &format!("{base}/api/v1/workspaces/default/members/bo"),
        &[("Authorization", &a)],
        Some(json!({ "role": "editor" })),
    )
    .await;
    send(
        "POST",
        &format!("{base}/api/v1/collections"),
        &[("Authorization", &b)],
        Some(json!({ "id": "shop" })),
    )
    .await;
    send(
        "PUT",
        &format!("{base}/api/v1/collections/shop/secrets/shared/token"),
        &[("Authorization", &b)],
        Some(json!({ "value": "s3cr3t-value" })),
    )
    .await;

    let events = |token: String| async move {
        let r = send(
            "GET",
            &format!("{base}/api/v1/audit"),
            &[("Authorization", &token)],
            None,
        )
        .await;
        json_of(&r)["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| {
                format!(
                    "{} {} {}",
                    e["action"].as_str().unwrap(),
                    e["username"].as_str().unwrap_or("-"),
                    e["target"].as_str().unwrap_or("")
                )
            })
            .collect::<Vec<_>>()
    };
    // Ana owns `default`: she sees Bo's changes there, but not Bo's own logins or workspace.
    let seen_by_ana = events(a.clone()).await;
    assert_eq!(
        seen_by_ana,
        [
            "secret.set bo shop/shared/token",
            "collection.created bo shop",
            "workspace.member_set ana bo: editor",
        ]
    );
    let seen_by_bo = events(b.clone()).await;
    assert!(
        seen_by_bo.contains(&"auth.login bo ".to_owned()),
        "{seen_by_bo:?}"
    );
    assert!(
        seen_by_bo.contains(&"workspace.created bo ".to_owned()),
        "{seen_by_bo:?}"
    );
    assert!(
        !seen_by_bo
            .iter()
            .any(|e| e.starts_with("auth.login_failed")),
        "anonymous events are for the server's administrator"
    );
    // The command line sees everything; no secret value is ever recorded.
    let all = srv.store.audit_log(None, None, 100).await.unwrap();
    assert!(
        all.iter()
            .any(|e| e.action == "auth.login_failed" && e.target.as_deref() == Some("bo"))
    );
    assert!(all.iter().all(|e| e.ip.as_deref() == Some("127.0.0.1")));
    assert!(!format!("{all:?}").contains("s3cr3t"));
}
