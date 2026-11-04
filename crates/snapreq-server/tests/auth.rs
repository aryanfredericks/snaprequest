//! Accounts, sessions and API tokens end to end (PLAN P8.1): two users on one server cannot see
//! each other's history or runs.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::PathBuf;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::config::{AuthConfig, AuthMode};
use snapreq_server::store::Retention;
use snapreq_server::store::Store;
use snapreq_server::{Config, Server};

struct Srv {
    addr: std::net::SocketAddr,
    store: Store,
    workspace: PathBuf,
}

async fn start(tag: &str, mode: AuthMode) -> Srv {
    let root = std::env::temp_dir().join(format!("snapreq-auth-{tag}-{}", std::process::id()));
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
            ..AuthConfig::default()
        },
    };
    let server = Server::bind(config).await.unwrap();
    let addr = server.local_addr().unwrap();
    let store = server.store().clone();
    tokio::spawn(server.run(std::future::pending()));
    Srv {
        addr,
        store,
        workspace: root.join("workspace"),
    }
}

/// A caller: a browser (cookie jar, sends the CSRF header) or a script with a token.
struct Caller {
    base: String,
    client: Client,
    csrf: bool,
    token: Option<String>,
}

impl Caller {
    fn browser(srv: &Srv) -> Self {
        Self {
            base: format!("http://{}", srv.addr),
            client: Client::default(),
            csrf: true,
            token: None,
        }
    }

    fn script(srv: &Srv, token: Option<&str>) -> Self {
        Self {
            csrf: false,
            token: token.map(str::to_owned),
            ..Self::browser(srv)
        }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = Request::new(
            http::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}/api/v1{path}", self.base),
        );
        if self.csrf {
            req = req.header("X-SnapReq-CSRF", "1");
        }
        if let Some(t) = &self.token {
            req = req.header("Authorization", format!("Bearer {t}"));
        }
        if let Some(b) = body {
            req.body = Body::Json {
                content: b.to_string(),
            };
        }
        let resp = self.client.execute(&req).await.unwrap();
        let doc = if resp.body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&resp.body).unwrap_or_else(|_| json!(resp.text()))
        };
        (resp.status, doc)
    }

    async fn login(&self, user: &str, password: &str) -> (u16, Value) {
        self.call(
            "POST",
            "/auth/login",
            Some(json!({ "username": user, "password": password })),
        )
        .await
    }

    /// Executes a request against the server's own health check, which lands in history.
    async fn execute(&self, srv: &Srv) -> i64 {
        let (status, doc) = self
            .call(
                "POST",
                "/requests/execute",
                Some(json!({ "request": { "method": "GET", "url": format!("http://{}/healthz", srv.addr) } })),
            )
            .await;
        assert_eq!(status, 200, "{doc}");
        doc["history_id"].as_i64().unwrap()
    }
}

fn kind(doc: &Value) -> &str {
    doc["error"]["kind"].as_str().unwrap_or("")
}

#[tokio::test]
async fn without_accounts_everyone_is_the_single_user() {
    let srv = start("none", AuthMode::None).await;
    let anyone = Caller::script(&srv, None);
    assert_eq!(
        anyone.call("GET", "/auth/status", None).await,
        (200, json!({ "mode": "none", "user": null }))
    );
    let id = anyone.execute(&srv).await;
    assert_eq!(
        anyone.call("GET", &format!("/history/{id}"), None).await.0,
        200
    );
    let (status, doc) = anyone.login("ana", "whatever-pass").await;
    assert_eq!((status, kind(&doc)), (409, "conflict"));
    assert_eq!(anyone.call("GET", "/auth/tokens", None).await.0, 409);
}

#[tokio::test]
async fn two_users_cannot_see_each_others_history_and_runs() {
    let srv = start("two", AuthMode::Accounts).await;
    srv.store.create_user("ana", "ana-password").await.unwrap();
    srv.store.create_user("bo", "bo-password!").await.unwrap();

    // Nothing but the public routes without logging in.
    let stranger = Caller::browser(&srv);
    let (status, doc) = stranger.call("GET", "/history", None).await;
    assert_eq!((status, kind(&doc)), (401, "unauthorized"));
    assert_eq!(stranger.call("GET", "/collections", None).await.0, 401);
    assert_eq!(
        stranger.call("GET", "/auth/status", None).await,
        (200, json!({ "mode": "accounts", "user": null }))
    );
    assert_eq!(stranger.call("GET", "/version", None).await.0, 200);
    let (status, doc) = stranger.login("ana", "wrong-password").await;
    assert_eq!((status, kind(&doc)), (401, "unauthorized"));
    let (status, _) = stranger.login("nobody", "ana-password").await;
    assert_eq!(status, 401);

    let ana = Caller::browser(&srv);
    let (status, doc) = ana.login("ANA", "ana-password").await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["user"]["username"], "ana", "user names ignore case");
    assert_eq!(
        ana.call("GET", "/auth/status", None).await.1["user"]["username"],
        "ana"
    );
    let bo = Caller::browser(&srv);
    assert_eq!(bo.login("bo", "bo-password!").await.0, 200);

    // Each user's history and runs are their own.
    let ana_entry = ana.execute(&srv).await;
    let bo_entry = bo.execute(&srv).await;
    let list = |doc: Value| -> Vec<i64> {
        doc["entries"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| e["id"].as_i64().unwrap())
            .collect()
    };
    assert_eq!(list(ana.call("GET", "/history", None).await.1), [ana_entry]);
    assert_eq!(list(bo.call("GET", "/history", None).await.1), [bo_entry]);
    assert_eq!(
        bo.call("GET", &format!("/history/{ana_entry}"), None)
            .await
            .0,
        404
    );
    assert_eq!(
        bo.call("DELETE", &format!("/history/{ana_entry}"), None)
            .await
            .0,
        404
    );
    assert_eq!(bo.call("DELETE", "/history", None).await.1["deleted"], 1);
    assert_eq!(list(ana.call("GET", "/history", None).await.1), [ana_entry]);

    // Collections are shared until workspaces exist (P8.2); runs are not.
    assert_eq!(
        ana.call("POST", "/collections", Some(json!({ "id": "app" })))
            .await
            .0,
        201
    );
    std::fs::write(
        srv.workspace.join("app/ping.toml"),
        format!("method = \"GET\"\nurl = \"http://{}/healthz\"\n", srv.addr),
    )
    .unwrap();
    let (status, run) = ana
        .call("POST", "/runs", Some(json!({ "collection": "app" })))
        .await;
    assert_eq!(status, 200, "{run}");
    let run_id = run["id"].as_i64().unwrap();
    assert_eq!(
        ana.call("GET", &format!("/runs/{run_id}"), None).await.0,
        200
    );
    assert_eq!(
        bo.call("GET", &format!("/runs/{run_id}"), None).await.0,
        404
    );
    assert_eq!(
        bo.call("GET", &format!("/runs/{run_id}/report/html"), None)
            .await
            .0,
        404
    );
    assert_eq!(bo.call("GET", "/runs", None).await.1, json!({ "runs": [] }));

    // Logging out ends the session.
    assert_eq!(bo.call("POST", "/auth/logout", None).await.0, 204);
    assert_eq!(bo.call("GET", "/history", None).await.0, 401);
}

#[tokio::test]
async fn cookie_requests_that_change_things_need_the_csrf_header() {
    let srv = start("csrf", AuthMode::Accounts).await;
    srv.store.create_user("ana", "ana-password").await.unwrap();
    let mut ana = Caller::browser(&srv);
    ana.csrf = false;
    let (status, doc) = ana.login("ana", "ana-password").await;
    assert_eq!(
        (status, kind(&doc)),
        (403, "forbidden"),
        "login needs it too"
    );
    ana.csrf = true;
    assert_eq!(ana.login("ana", "ana-password").await.0, 200);
    ana.csrf = false;
    // Reading is fine, changing is not.
    assert_eq!(ana.call("GET", "/history", None).await.0, 200);
    let (status, doc) = ana.call("DELETE", "/history", None).await;
    assert_eq!((status, kind(&doc)), (403, "forbidden"));
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .contains("x-snapreq-csrf")
    );
}

#[tokio::test]
async fn api_tokens() {
    let srv = start("tokens", AuthMode::Accounts).await;
    srv.store.create_user("ana", "ana-password").await.unwrap();
    let ana = Caller::browser(&srv);
    ana.login("ana", "ana-password").await;
    let (status, doc) = ana
        .call(
            "POST",
            "/auth/tokens",
            Some(json!({ "name": "ci", "expires_in_days": 30 })),
        )
        .await;
    assert_eq!(status, 201, "{doc}");
    let secret = doc["secret"].as_str().unwrap().to_owned();
    assert!(secret.starts_with("snapreq_pat_"));
    assert_eq!(doc["token"]["hint"], secret[secret.len() - 4..]);
    assert!(doc["token"]["expires_at"].is_i64());
    let token_id = doc["token"]["id"].as_i64().unwrap();
    let (_, list) = ana.call("GET", "/auth/tokens", None).await;
    assert_eq!(list["tokens"][0]["name"], "ci");
    assert!(
        !list.to_string().contains(&secret),
        "the secret is shown once"
    );

    // A script with the token is Ana, without a CSRF header.
    let ci = Caller::script(&srv, Some(&secret));
    let id = ci.execute(&srv).await;
    assert_eq!(
        ana.call("GET", &format!("/history/{id}"), None).await.0,
        200
    );
    assert_eq!(
        ci.call("DELETE", &format!("/history/{id}"), None).await.0,
        204
    );
    let (_, list) = ana.call("GET", "/auth/tokens", None).await;
    assert!(list["tokens"][0]["last_used_at"].is_i64());

    let (status, doc) = Caller::script(&srv, Some("snapreq_pat_wrong"))
        .call("GET", "/history", None)
        .await;
    assert_eq!((status, kind(&doc)), (401, "unauthorized"));
    assert_eq!(
        ana.call("POST", "/auth/tokens", Some(json!({ "name": " " })))
            .await
            .0,
        422
    );
    assert_eq!(
        ana.call(
            "POST",
            "/auth/tokens",
            Some(json!({ "name": "x", "expires_in_days": 0 }))
        )
        .await
        .0,
        422
    );

    // Other users cannot revoke it; its owner can.
    srv.store.create_user("bo", "bo-password!").await.unwrap();
    let bo = Caller::browser(&srv);
    bo.login("bo", "bo-password!").await;
    assert_eq!(
        bo.call("DELETE", &format!("/auth/tokens/{token_id}"), None)
            .await
            .0,
        404
    );
    assert_eq!(
        ana.call("DELETE", &format!("/auth/tokens/{token_id}"), None)
            .await
            .0,
        204
    );
    assert_eq!(ci.call("GET", "/history", None).await.0, 401);
}

#[tokio::test]
async fn changing_the_password_ends_other_sessions() {
    let srv = start("password", AuthMode::Accounts).await;
    srv.store.create_user("ana", "ana-password").await.unwrap();
    let laptop = Caller::browser(&srv);
    let phone = Caller::browser(&srv);
    laptop.login("ana", "ana-password").await;
    phone.login("ana", "ana-password").await;
    let (status, doc) = laptop
        .call(
            "POST",
            "/auth/password",
            Some(json!({ "current": "nope-nope", "new": "new-password" })),
        )
        .await;
    assert_eq!((status, kind(&doc)), (403, "forbidden"));
    let (status, doc) = laptop
        .call(
            "POST",
            "/auth/password",
            Some(json!({ "current": "ana-password", "new": "short" })),
        )
        .await;
    assert_eq!((status, kind(&doc)), (422, "invalid"));
    let (status, _) = laptop
        .call(
            "POST",
            "/auth/password",
            Some(json!({ "current": "ana-password", "new": "new-password" })),
        )
        .await;
    assert_eq!(status, 204);
    assert_eq!(laptop.call("GET", "/history", None).await.0, 200);
    assert_eq!(phone.call("GET", "/history", None).await.0, 401);
    assert_eq!(
        Caller::browser(&srv).login("ana", "ana-password").await.0,
        401
    );
    assert_eq!(
        Caller::browser(&srv).login("ana", "new-password").await.0,
        200
    );

    // Deleting the user ends everything.
    assert!(srv.store.delete_user("ana").await.unwrap());
    assert_eq!(laptop.call("GET", "/history", None).await.0, 401);
}

#[tokio::test]
async fn the_websocket_needs_a_login_and_this_origin() {
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::{Message, http::HeaderValue};

    let srv = start("ws", AuthMode::Accounts).await;
    srv.store.create_user("ana", "ana-password").await.unwrap();
    let url = format!("ws://{}/api/v1/ws", srv.addr);
    let err = tokio_tungstenite::connect_async(&url).await.unwrap_err();
    assert!(err.to_string().contains("401"), "{err}");

    let (token, secret) = srv.store.create_token(1, "ws", None).await.unwrap();
    assert_eq!(token.name, "ws");
    let mut req = url.as_str().into_client_request().unwrap();
    req.headers_mut().insert(
        "authorization",
        HeaderValue::from_str(&format!("Bearer {secret}")).unwrap(),
    );
    req.headers_mut()
        .insert("origin", HeaderValue::from_static("http://evil.example"));
    let err = tokio_tungstenite::connect_async(req.clone())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("403"), "{err}");

    req.headers_mut().insert(
        "origin",
        HeaderValue::from_str(&format!("http://{}", srv.addr)).unwrap(),
    );
    let (mut ws, _) = tokio_tungstenite::connect_async(req).await.unwrap();
    let msg = json!({ "type": "execute", "id": 1, "request": { "method": "GET", "url": format!("http://{}/healthz", srv.addr) } });
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();
    let result = loop {
        let v: Value = match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            _ => continue,
        };
        if v["type"] == "result" {
            break v;
        }
    };
    let id = result["result"]["history_id"].as_i64().unwrap();
    let ana = Caller::script(&srv, Some(&secret));
    assert_eq!(
        ana.call("GET", &format!("/history/{id}"), None).await.0,
        200
    );

    // Revoking the token ends the open connection at its next job.
    assert!(srv.store.delete_token(1, token.id).await.unwrap());
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();
    let v: Value = loop {
        if let Message::Text(t) = ws.next().await.unwrap().unwrap() {
            break serde_json::from_str(&t).unwrap();
        }
    };
    assert_eq!(v["error"]["kind"], "unauthorized");
}

#[tokio::test]
async fn the_ui_and_health_check_need_no_login() {
    let srv = start("public", AuthMode::Accounts).await;
    let client = Client::default();
    for path in ["/healthz", "/", "/history/1"] {
        let resp = client
            .execute(&Request::get(format!("http://{}{path}", srv.addr)))
            .await
            .unwrap();
        assert_eq!(resp.status, 200, "{path}");
    }
}

#[tokio::test]
async fn sessions_are_cookies_the_page_cannot_read() {
    let srv = start("cookie", AuthMode::Accounts).await;
    srv.store.create_user("ana", "ana-password").await.unwrap();
    let mut req = Request::new(
        http::Method::POST,
        format!("http://{}/api/v1/auth/login", srv.addr),
    )
    .header("X-SnapReq-CSRF", "1");
    req.body = Body::Json {
        content: json!({ "username": "ana", "password": "ana-password" }).to_string(),
    };
    let resp = Client::default().execute(&req).await.unwrap();
    let cookie = resp.header("set-cookie").unwrap();
    assert!(cookie.starts_with("snapreq_session="), "{cookie}");
    for attr in ["HttpOnly", "SameSite=Strict", "Path=/", "Max-Age=1209600"] {
        assert!(cookie.contains(attr), "{cookie}");
    }
    assert!(!cookie.contains("Secure"));
    assert!(!String::from_utf8_lossy(&resp.body).contains("snapreq_session"));
}
