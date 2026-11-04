//! The REST API end to end: a server on a random port, a target API next to it.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::store::Retention;
use snapreq_server::{Config, Server};
use tokio::net::TcpListener;

/// The API under test: echoes the request; `/login` returns a token.
async fn target() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let svc = service_fn(|req: hyper::Request<hyper::body::Incoming>| async move {
                    let body = if req.uri().path() == "/login" {
                        r#"{"token": "srv-secret-token-1"}"#.to_owned()
                    } else {
                        let mut out = format!("{} {}\n", req.method(), req.uri());
                        for (n, v) in req.headers() {
                            out.push_str(&format!("{n}: {}\n", v.to_str().unwrap()));
                        }
                        let b = req.into_body().collect().await.unwrap().to_bytes();
                        out.push_str(&String::from_utf8_lossy(&b));
                        out
                    };
                    Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from(body))))
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    addr
}

struct Api {
    base: String,
    client: Client,
    workspace: PathBuf,
}

impl Api {
    async fn start(tag: &str) -> Self {
        Self::start_with(tag, Default::default()).await
    }

    async fn start_with(tag: &str, outbound: snapreq_server::outbound::OutboundPolicy) -> Self {
        let root = std::env::temp_dir().join(format!("snapreq-api-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir: root.join("data"),
            workspace: root.join("workspace"),
            allow_remote: false,
            retention: Retention::default(),
            outbound,
            auth: snapreq_server::config::AuthConfig::default(),
            oidc: None,
        };
        let server = Server::bind(config).await.unwrap();
        let addr = server.local_addr().unwrap();
        tokio::spawn(server.run(std::future::pending()));
        Self {
            base: format!("http://{addr}/api/v1"),
            client: Client::default(),
            workspace: root.join("workspace"),
        }
    }

    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let mut req = Request::new(
            http::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{}{path}", self.base),
        );
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
}

#[tokio::test]
async fn collections_requests_flows_and_environments() {
    let api = Api::start("crud").await;
    assert_eq!(
        api.call("GET", "/collections", None).await,
        (200, json!({ "collections": [] }))
    );

    let (status, doc) = api
        .call(
            "POST",
            "/collections",
            Some(json!({ "id": "shop", "name": "Shop API" })),
        )
        .await;
    assert_eq!(status, 201, "{doc}");
    assert!(api.workspace.join("shop/snapreq.toml").is_file());
    assert_eq!(
        api.call("POST", "/collections", Some(json!({ "id": "shop" })))
            .await
            .0,
        409
    );
    let (status, doc) = api
        .call("POST", "/collections", Some(json!({ "id": "../evil" })))
        .await;
    assert_eq!(
        (status, doc["error"]["kind"].clone()),
        (400, json!("bad_request"))
    );

    let request = json!({ "name": "Create user", "method": "POST", "url": "{{base}}/users", "headers": { "X-Team": "shop" } });
    let (status, doc) = api
        .call(
            "PUT",
            "/collections/shop/requests/users/create",
            Some(request),
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    let text = std::fs::read_to_string(api.workspace.join("shop/users/create.toml")).unwrap();
    assert!(
        text.starts_with("name = \"Create user\"\nmethod = \"POST\"\nurl = \"{{base}}/users\"\n"),
        "{text}"
    );

    let (_, doc) = api
        .call("GET", "/collections/shop/requests/users/create", None)
        .await;
    assert_eq!(doc["file"]["headers"]["X-Team"], "shop");
    let (_, doc) = api.call("GET", "/collections/shop", None).await;
    assert_eq!(doc["items"][0]["type"], "folder");
    assert_eq!(
        doc["items"][0]["items"][0],
        json!({ "type": "request", "key": "users/create", "name": "Create user", "method": "POST" })
    );

    // Invalid files and unsafe keys are refused before anything is written.
    let (status, doc) = api
        .call(
            "PUT",
            "/collections/shop/requests/bad",
            Some(json!({ "url": " " })),
        )
        .await;
    assert_eq!(status, 422, "{doc}");
    assert_eq!(
        api.call(
            "PUT",
            "/collections/shop/requests/x",
            Some(json!({ "url": "u", "nope": 1 }))
        )
        .await
        .0,
        422
    );
    for key in [
        "..%2F..%2Fx",
        ".secrets/dev",
        "environments/dev",
        "users/folder",
    ] {
        let (status, _) = api
            .call(
                "PUT",
                &format!("/collections/shop/requests/{key}"),
                Some(json!({ "url": "u" })),
            )
            .await;
        assert!(status == 400 || status == 404, "{key}: {status}");
    }
    assert!(!api.workspace.join("x").exists());

    let env = json!({ "base": "http://localhost", "token": { "secret": true } });
    assert_eq!(
        api.call(
            "PUT",
            "/collections/shop/environments/dev",
            Some(env.clone())
        )
        .await
        .0,
        200
    );
    let (_, doc) = api
        .call("GET", "/collections/shop/environments/dev", None)
        .await;
    assert_eq!(doc["vars"], env);

    let flow = json!({ "name": "Onboard", "step": [{ "request": "users/create" }, { "url": "{{base}}/x", "method": "DELETE" }] });
    assert_eq!(
        api.call("PUT", "/collections/shop/flows/users/onboard", Some(flow))
            .await
            .0,
        200
    );
    assert!(api.workspace.join("shop/users/onboard.flow.toml").is_file());
    let (_, doc) = api.call("GET", "/collections/shop", None).await;
    assert_eq!(
        doc["flows"],
        json!([{ "key": "users/onboard", "name": "Onboard", "steps": 2 }])
    );
    assert_eq!(doc["environments"], json!(["dev"]));
    let (status, doc) = api
        .call(
            "PUT",
            "/collections/shop/flows/empty",
            Some(json!({ "step": [] })),
        )
        .await;
    assert_eq!(status, 422, "{doc}");

    let (_, doc) = api.call("GET", "/collections", None).await;
    assert_eq!(
        doc["collections"][0],
        json!({ "id": "shop", "name": "Shop API", "description": null, "requests": 1, "flows": 1 })
    );

    assert_eq!(
        api.call("DELETE", "/collections/shop/flows/users/onboard", None)
            .await
            .0,
        204
    );
    assert_eq!(
        api.call("DELETE", "/collections/shop/requests/users/create", None)
            .await
            .0,
        204
    );
    assert_eq!(
        api.call("DELETE", "/collections/shop/requests/users/create", None)
            .await
            .0,
        404
    );
    assert_eq!(
        api.call("DELETE", "/collections/shop/environments/dev", None)
            .await
            .0,
        204
    );
    assert_eq!(api.call("DELETE", "/collections/shop", None).await.0, 204);
    assert_eq!(api.call("GET", "/collections/shop", None).await.0, 404);
}

#[tokio::test]
async fn execute_history_and_runs() {
    let api = Api::start("exec").await;
    let target = target().await;
    let base = format!("http://{target}");

    // A raw request.
    let raw = json!({ "request": { "method": "GET", "url": format!("{base}/hello"), "headers": [{ "name": "X-Api-Key", "value": "raw-key-123456" }] } });
    let (status, doc) = api.call("POST", "/requests/execute", Some(raw)).await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["response"]["status"], 200);
    let first = doc["history_id"].as_i64().unwrap();
    let (status, doc) = api
        .call(
            "POST",
            "/requests/execute",
            Some(json!({ "request": { "method": "GET", "url": "nope" } })),
        )
        .await;
    assert_eq!(status, 422, "{doc}");
    let (_, doc) = api
        .call(
            "POST",
            "/requests/execute",
            Some(json!({ "request": { "method": "GET", "url": "http://127.0.0.1:1/" } })),
        )
        .await;
    assert_eq!(doc["error"]["phase"], "connect");

    // A collection request with an environment, variables, checks and a secret capture.
    api.call("POST", "/collections", Some(json!({ "id": "app" })))
        .await;
    api.call(
        "PUT",
        "/collections/app/environments/test",
        Some(json!({ "base": base })),
    )
    .await;
    api.call(
        "PUT",
        "/collections/app/requests/login",
        Some(json!({ "method": "POST", "url": "{{base}}/login", "capture": { "token": { "json": "$.token", "secret": true } }, "assert": { "status": 200, "json": { "$.token": { "type": "string" } } } })),
    )
    .await;
    api.call(
        "PUT",
        "/collections/app/requests/me",
        Some(json!({ "url": "{{base}}/me/{{who}}", "auth": { "type": "bearer", "token": "{{token}}" }, "assert": { "body": { "contains": "Bearer" } } })),
    )
    .await;
    let (status, doc) = api
        .call(
            "POST",
            "/requests/execute",
            Some(json!({ "collection": "app", "key": "login", "env": "test" })),
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["assertions"][0]["passed"], true);
    assert_eq!(
        doc["captures"][0],
        json!({ "name": "token", "value": "<redacted>", "secret": true })
    );
    let login = doc["history_id"].as_i64().unwrap();
    let (status, doc) = api.call("POST", "/requests/execute", Some(json!({ "collection": "app", "key": "me", "env": "test", "vars": { "who": "ann" } }))).await;
    assert_eq!(
        status, 422,
        "a single execution has no earlier captures: {doc}"
    );
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .contains("`token`")
    );

    // History: newest first, searchable, nothing secret stored.
    let (_, doc) = api.call("GET", "/history?limit=10", None).await;
    let entries = doc["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["request_key"], "login");
    let (_, doc) = api.call("GET", "/history?url=hello", None).await;
    assert_eq!(doc["entries"].as_array().unwrap().len(), 1);
    let (_, doc) = api.call("GET", &format!("/history/{first}"), None).await;
    assert_eq!(doc["request"]["headers"][0]["value"], "<redacted>");
    let (_, doc) = api.call("GET", &format!("/history/{login}"), None).await;
    assert!(!doc.to_string().contains("srv-secret-token-1"), "{doc}");
    assert_eq!(api.call("GET", "/history/9999", None).await.0, 404);

    // Runs: a flow uses the captured token in the next step.
    api.call(
        "PUT",
        "/collections/app/flows/session",
        Some(json!({ "step": [{ "request": "login" }, { "request": "me", "vars": { "who": "bob" } }] })),
    )
    .await;
    let (status, doc) = api
        .call(
            "POST",
            "/runs",
            Some(json!({ "collection": "app", "target": "session", "env": "test" })),
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["passed"], true, "{doc}");
    let run = doc["id"].as_i64().unwrap();
    let (_, doc) = api.call("POST", "/runs", Some(json!({ "collection": "app", "target": "session", "env": "test", "data": [{ "who": "x" }, { "who": "y" }] }))).await;
    assert_eq!(doc["iterations"].as_array().unwrap().len(), 2);
    let (_, doc) = api.call("GET", "/runs", None).await;
    assert_eq!(doc["runs"].as_array().unwrap().len(), 2);
    assert_eq!(doc["runs"][1]["target_key"], "app/session");
    let (_, doc) = api.call("GET", &format!("/runs/{run}"), None).await;
    assert!(
        doc["iterations"][0]["steps"][1]["response"]["trace"].is_array(),
        "stored runs keep traces"
    );
    assert!(!doc.to_string().contains("srv-secret-token-1"));
    let (status, doc) = api
        .call(
            "POST",
            "/runs",
            Some(json!({ "collection": "app", "target": "nope" })),
        )
        .await;
    assert_eq!(status, 404, "{doc}");
    assert_eq!(
        api.call(
            "POST",
            "/runs",
            Some(json!({ "collection": "app", "data": [{}], "iterations": 2 }))
        )
        .await
        .0,
        400
    );
}

#[tokio::test]
async fn websocket_streams_traces_progress_and_results() {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let api = Api::start("ws").await;
    let target = target().await;
    let base = format!("http://{target}");
    api.call("POST", "/collections", Some(json!({ "id": "app" })))
        .await;
    api.call(
        "PUT",
        "/collections/app/environments/test",
        Some(json!({ "base": base })),
    )
    .await;
    api.call("PUT", "/collections/app/requests/login", Some(json!({ "method": "POST", "url": "{{base}}/login", "capture": { "token": { "json": "$.token", "secret": true } } }))).await;
    api.call(
        "PUT",
        "/collections/app/requests/me",
        Some(json!({ "url": "{{base}}/me", "auth": { "type": "bearer", "token": "{{token}}" } })),
    )
    .await;
    api.call(
        "PUT",
        "/collections/app/flows/session",
        Some(json!({ "step": [{ "request": "login" }, { "request": "me" }] })),
    )
    .await;

    let url = format!("{}/ws", api.base.replacen("http://", "ws://", 1));
    let (mut ws, _) = tokio_tungstenite::connect_async(url).await.unwrap();
    // Execute: trace events arrive before the result.
    let msg = json!({ "type": "execute", "id": "a", "request": { "method": "GET", "url": format!("{base}/x") } });
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();
    let mut events = Vec::new();
    loop {
        let v: Value = match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            _ => continue,
        };
        let done = v["type"] == "result" || v["type"] == "error";
        events.push(v);
        if done {
            break;
        }
    }
    let kinds: Vec<String> = events
        .iter()
        .map(|e| e["type"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(kinds.last().unwrap(), "result");
    assert!(
        kinds.iter().filter(|k| *k == "trace").count() >= 5,
        "{kinds:?}"
    );
    assert_eq!(events[0]["id"], "a");
    assert_eq!(events[0]["entry"]["event"], "connect_start");
    assert_eq!(events.last().unwrap()["result"]["response"]["status"], 200);

    // Run: progress per request, the token captured by `login` is hidden in later traces.
    let msg =
        json!({ "type": "run", "id": 7, "collection": "app", "target": "session", "env": "test" });
    ws.send(Message::Text(msg.to_string().into()))
        .await
        .unwrap();
    let mut events = Vec::new();
    loop {
        let v: Value = match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            _ => continue,
        };
        let done = v["type"] == "result" || v["type"] == "error";
        events.push(v);
        if done {
            break;
        }
    }
    let progress: Vec<(String, String)> = events
        .iter()
        .filter(|e| e["type"] == "progress")
        .map(|e| {
            (
                e["event"].as_str().unwrap().to_owned(),
                e["key"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(
        progress,
        [
            ("request_started".to_owned(), "login".to_owned()),
            ("request_done".to_owned(), "login".to_owned()),
            ("request_started".to_owned(), "me".to_owned()),
            ("request_done".to_owned(), "me".to_owned()),
        ]
    );
    let all = serde_json::to_string(&events).unwrap();
    assert!(!all.contains("srv-secret-token-1"), "{all}");
    assert!(all.contains("Bearer <redacted>"));
    let result = &events.last().unwrap()["result"];
    assert_eq!(result["passed"], true, "{result}");
    assert!(
        result["id"].as_i64().is_some(),
        "runs over the socket are stored too"
    );

    // Bad messages get an error and the connection stays usable.
    ws.send(Message::Text("[1]".into())).await.unwrap();
    ws.send(Message::Text(json!({ "type": "dance" }).to_string().into()))
        .await
        .unwrap();
    let mut errors = Vec::new();
    while errors.len() < 2 {
        if let Message::Text(t) = ws.next().await.unwrap().unwrap() {
            errors.push(serde_json::from_str::<Value>(&t).unwrap());
        }
    }
    assert_eq!(errors[0]["error"]["message"], "expected a JSON object");
    assert!(
        errors[1]["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown message type `dance`")
    );
}

#[tokio::test]
async fn the_outbound_policy_blocks_executed_requests() {
    let policy = snapreq_server::outbound::OutboundPolicy {
        deny: vec!["127.0.0.0/8".parse().unwrap()],
        ..Default::default()
    };
    let api = Api::start_with("ssrf", policy).await;
    let target = target().await;
    let (status, doc) = api
        .call(
            "POST",
            "/requests/execute",
            Some(json!({ "request": { "method": "GET", "url": format!("http://{target}/") } })),
        )
        .await;
    assert_eq!(status, 200, "the API itself is reachable: {doc}");
    assert_eq!(doc["error"]["phase"], "connect");
    assert_eq!(
        doc["error"]["message"],
        "blocked by the outbound request policy: 127.0.0.1: denied by rule `127.0.0.0/8`"
    );
    let (_, doc) = api.call("GET", "/history", None).await;
    assert!(
        doc["entries"][0]["error"]
            .as_str()
            .unwrap()
            .contains("blocked by the outbound request policy")
    );
}

#[tokio::test]
async fn unsaved_edits_wire_capture_and_variables() {
    let api = Api::start("unsaved").await;
    let target = target().await;
    let base = format!("http://{target}");
    api.call("POST", "/collections", Some(json!({ "id": "app" })))
        .await;
    api.call(
        "PUT",
        "/collections/app",
        Some(json!({ "format": 1, "name": "App", "vars": { "base": "http://unused", "token": { "secret": true } }, "headers": { "X-From": "collection" } })),
    )
    .await;
    api.call(
        "PUT",
        "/collections/app/environments/dev",
        Some(json!({ "base": base })),
    )
    .await;
    api.call(
        "PUT",
        "/collections/app/requests/users/list",
        Some(json!({ "url": "{{base}}/saved" })),
    )
    .await;

    // The edited file is sent with the collection's headers and the environment.
    let (status, doc) = api
        .call(
            "POST",
            "/requests/execute",
            Some(json!({ "collection": "app", "key": "users/list", "env": "dev", "capture_wire": true,
                         "file": { "method": "PUT", "url": "{{base}}/edited/{{who}}", "vars": { "who": "me" } } })),
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    let body = doc["response"]["body"]["data"].as_str().unwrap();
    assert!(body.starts_with("PUT /edited/me"), "{body}");
    assert!(body.contains("x-from: collection"), "{body}");
    let trace = doc["response"]["trace"].as_array().unwrap();
    assert!(trace.iter().any(|e| e["event"] == "wire"), "no wire event");
    // Nothing was saved.
    let (_, saved) = api
        .call("GET", "/collections/app/requests/users/list", None)
        .await;
    assert_eq!(saved["file"]["url"], "{{base}}/saved");

    // A new, unsaved request in an existing folder; invalid files and folders are refused.
    let (status, doc) = api
        .call(
            "POST",
            "/requests/execute",
            Some(json!({ "collection": "app", "key": "users/new", "env": "dev", "file": { "url": "{{base}}/new" } })),
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["response"]["status"], 200);
    let (status, _) = api
        .call(
            "POST",
            "/requests/execute",
            Some(json!({ "collection": "app", "key": "nope/new", "file": { "url": "{{base}}/new" } })),
        )
        .await;
    assert_eq!(status, 404);
    let (status, _) = api
        .call(
            "POST",
            "/requests/execute",
            Some(json!({ "collection": "app", "key": "users/list", "file": { "url": "" } })),
        )
        .await;
    assert_eq!(status, 422);
    let (status, _) = api
        .call(
            "POST",
            "/requests/execute",
            Some(json!({ "request": { "method": "GET", "url": base }, "file": { "url": "x" } })),
        )
        .await;
    assert_eq!(status, 400);

    // Variables as the request sees them; secret values are never returned.
    let (status, doc) = api
        .call(
            "GET",
            "/collections/app/variables?key=users/list&env=dev",
            None,
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    let vars = doc["variables"].as_array().unwrap();
    let base_var = vars.iter().find(|v| v["name"] == "base").unwrap();
    assert_eq!(base_var["value"], base.as_str());
    assert_eq!(base_var["scope"], "environment `dev`");
    let token = vars.iter().find(|v| v["name"] == "token").unwrap();
    assert_eq!(token["secret"], true);
    assert_eq!(token["value"], Value::Null);
    let (status, _) = api
        .call("GET", "/collections/app/variables?env=nope", None)
        .await;
    assert_eq!(status, 404);
}

#[tokio::test]
async fn folders_moves_order_and_secrets() {
    let api = Api::start("tree").await;
    api.call("POST", "/collections", Some(json!({ "id": "app" })))
        .await;
    for key in ["a", "b", "c", "users/list"] {
        api.call(
            "PUT",
            &format!("/collections/app/requests/{key}"),
            Some(json!({ "url": "http://x/{{token}}" })),
        )
        .await;
    }
    let keys = |doc: &Value| -> Vec<String> {
        doc["items"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| i["key"].as_str().unwrap().to_owned())
            .collect()
    };

    // Folders: create with settings, read back, refuse a clash with a request.
    let (status, doc) = api
        .call(
            "PUT",
            "/collections/app/folders/admin",
            Some(json!({ "name": "Admin", "headers": { "X-Admin": "1" } })),
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    let (_, doc) = api
        .call("GET", "/collections/app/folders/admin", None)
        .await;
    assert_eq!(doc["file"]["name"], "Admin");
    let (status, _) = api
        .call("PUT", "/collections/app/folders/a", Some(json!({})))
        .await;
    assert_eq!(status, 409);
    let (status, _) = api.call("GET", "/collections/app/folders/nope", None).await;
    assert_eq!(status, 404);

    // Reorder at the root: `c` before `a`.
    let (status, doc) = api
        .call(
            "POST",
            "/collections/app/move",
            Some(json!({ "kind": "request", "from": "c", "place": { "before": "a" } })),
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    let (_, doc) = api.call("GET", "/collections/app", None).await;
    assert_eq!(keys(&doc), ["c", "a", "admin", "b", "users"]);

    // Move into a folder (renaming it), first among its items.
    let (status, doc) = api
        .call(
            "POST",
            "/collections/app/move",
            Some(json!({ "kind": "request", "from": "a", "to": "users", "name": "first", "place": { "before": "list" } })),
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["key"], "users/first");
    let (_, doc) = api.call("GET", "/collections/app", None).await;
    assert_eq!(keys(&doc), ["c", "admin", "b", "users"]);
    let users = &doc["items"][3]["items"];
    assert_eq!(users[0]["key"], "users/first");
    assert_eq!(users[1]["key"], "users/list");

    // A rename in place keeps the position.
    api.call(
        "POST",
        "/collections/app/move",
        Some(json!({ "kind": "request", "from": "c", "name": "cc" })),
    )
    .await;
    let (_, doc) = api.call("GET", "/collections/app", None).await;
    assert_eq!(keys(&doc), ["cc", "admin", "b", "users"]);

    // Folders move with their contents; not into themselves; clashes are refused.
    let (status, _) = api
        .call(
            "POST",
            "/collections/app/move",
            Some(json!({ "kind": "folder", "from": "users", "to": "users" })),
        )
        .await;
    assert_eq!(status, 400);
    let (status, doc) = api
        .call(
            "POST",
            "/collections/app/move",
            Some(json!({ "kind": "folder", "from": "users", "to": "admin" })),
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    let (status, _) = api
        .call("GET", "/collections/app/requests/admin/users/list", None)
        .await;
    assert_eq!(status, 200);
    let (status, _) = api
        .call(
            "POST",
            "/collections/app/move",
            Some(json!({ "kind": "request", "from": "b", "name": "cc" })),
        )
        .await;
    assert_eq!(status, 409);

    // Deleting removes the item from the order too.
    let (status, _) = api
        .call("DELETE", "/collections/app/requests/cc", None)
        .await;
    assert_eq!(status, 204);
    let (_, doc) = api.call("GET", "/collections/app", None).await;
    assert_eq!(doc["file"]["order"], json!(["admin", "b"]));
    let (status, _) = api
        .call("DELETE", "/collections/app/folders/admin", None)
        .await;
    assert_eq!(status, 204);
    let (_, doc) = api.call("GET", "/collections/app", None).await;
    assert_eq!(keys(&doc), ["b"]);

    // Secrets: stored in .secrets/, reported as set, never returned.
    api.call(
        "PUT",
        "/collections/app",
        Some(json!({ "format": 1, "name": "App", "vars": { "token": { "secret": true } } })),
    )
    .await;
    api.call("PUT", "/collections/app/environments/dev", Some(json!({})))
        .await;
    let (status, doc) = api
        .call(
            "PUT",
            "/collections/app/secrets/dev/token",
            Some(json!({ "value": "s3cr3t-value" })),
        )
        .await;
    assert_eq!(status, 204, "{doc}");
    let stored = std::fs::read_to_string(api.workspace.join("app/.secrets/dev.toml")).unwrap();
    assert!(stored.contains("s3cr3t-value"));
    assert!(api.workspace.join("app/.secrets/.gitignore").is_file());
    let (_, doc) = api
        .call("GET", "/collections/app/variables?env=dev", None)
        .await;
    let token = doc["variables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "token")
        .unwrap()
        .clone();
    assert_eq!(token["has_value"], true);
    assert_eq!(token["value"], Value::Null);
    assert!(!doc.to_string().contains("s3cr3t"));
    let (status, _) = api
        .call(
            "PUT",
            "/collections/app/secrets/nope/token",
            Some(json!({ "value": "x" })),
        )
        .await;
    assert_eq!(status, 404);
    let (status, _) = api
        .call(
            "PUT",
            "/collections/app/secrets/shared/token",
            Some(json!({ "value": "" })),
        )
        .await;
    assert_eq!(status, 400);
}

#[tokio::test]
async fn flows_run_by_steps_and_resume() {
    let api = Api::start("steps").await;
    let target = target().await;
    api.call("POST", "/collections", Some(json!({ "id": "app" })))
        .await;
    api.call(
        "PUT",
        "/collections/app",
        Some(json!({ "format": 1, "name": "App", "vars": { "base": format!("http://{target}") } })),
    )
    .await;
    let (status, doc) = api
        .call(
            "PUT",
            "/collections/app/flows/journey",
            Some(json!({ "step": [
                { "name": "Log in", "method": "POST", "url": "{{base}}/login", "capture": { "token": { "json": "$.token", "secret": true } } },
                { "name": "Me", "url": "{{base}}/me", "auth": { "type": "bearer", "token": "{{token}}" }, "assert": { "body": { "contains": "Bearer" } } },
                { "name": "Again", "url": "{{base}}/again", "auth": { "type": "bearer", "token": "{{token}}" }, "assert": { "body": { "contains": "Bearer" } } }
            ] })),
        )
        .await;
    assert_eq!(status, 200, "{doc}");

    // Step 1 only.
    let (status, first) = api
        .call(
            "POST",
            "/runs",
            Some(json!({ "collection": "app", "target": "journey", "to_step": 1, "capture_wire": true })),
        )
        .await;
    assert_eq!(status, 200, "{first}");
    assert_eq!(first["passed"], true, "{first}");
    assert_eq!(first["iterations"][0]["steps"].as_array().unwrap().len(), 1);
    assert_eq!(first["resumable_steps"], json!([1, 2]));
    assert_eq!(first["range"], json!([1, 1]));
    assert!(!first.to_string().contains("srv-secret-token-1"));
    let run = first["id"].as_i64().unwrap();
    // The stored report has the wire capture.
    let (_, stored) = api.call("GET", &format!("/runs/{run}"), None).await;
    assert!(stored.to_string().contains("\"wire\""), "{stored}");
    // Run options apply to every request (`run -k`, `-t`).
    let (status, doc) = api
        .call(
            "POST",
            "/runs",
            Some(json!({ "collection": "app", "target": "journey", "to_step": 1, "insecure": true, "timeout_ms": 5000 })),
        )
        .await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["passed"], true, "{doc}");

    // Reports as downloads, like `snapreq run --report`.
    for (format, needle) in [
        ("junit", "<testsuites"),
        ("html", "<!DOCTYPE html"),
        ("json", "\"format\": 1"),
    ] {
        let resp = api
            .client
            .execute(&Request::get(format!(
                "{}/runs/{run}/report/{format}",
                api.base
            )))
            .await
            .unwrap();
        assert_eq!(resp.status, 200, "{format}");
        let text = resp.text().unwrap().to_owned();
        assert!(
            text.to_lowercase().contains(&needle.to_lowercase()),
            "{format}: {text:.200}"
        );
        assert!(!text.contains("srv-secret-token-1"), "{format}");
        let disposition = resp
            .headers
            .iter()
            .find(|h| h.name == "content-disposition")
            .unwrap();
        assert!(disposition.value.starts_with("attachment"));
    }
    let (status, _) = api
        .call("GET", &format!("/runs/{run}/report/pdf"), None)
        .await;
    assert_eq!(status, 400);
    let (status, _) = api.call("GET", "/runs/999/report/html", None).await;
    assert_eq!(status, 404);

    // Step 2 with step 1's captured token; then 2–3 again from the same state.
    for (from, to, count) in [(2, Some(2), 1), (2, None, 2)] {
        let mut body =
            json!({ "collection": "app", "target": "journey", "from_step": from, "resume": run });
        if let Some(to) = to {
            body["to_step"] = json!(to);
        }
        let (status, doc) = api.call("POST", "/runs", Some(body)).await;
        assert_eq!(status, 200, "{doc}");
        assert_eq!(doc["passed"], true, "{doc}");
        let steps = doc["iterations"][0]["steps"].as_array().unwrap();
        assert_eq!(steps.len(), count);
        assert_eq!(steps[0]["number"], 2);
        assert!(!doc.to_string().contains("srv-secret-token-1"));
    }

    // Starting later without a state, unknown runs, bad ranges and non-flows are refused.
    api.call(
        "PUT",
        "/collections/app/requests/ping",
        Some(json!({ "url": "{{base}}/ping" })),
    )
    .await;
    for (body, want) in [
        (
            json!({ "collection": "app", "target": "journey", "from_step": 2 }),
            400,
        ),
        (
            json!({ "collection": "app", "target": "journey", "from_step": 2, "resume": 999 }),
            409,
        ),
        (
            json!({ "collection": "app", "target": "journey", "from_step": 4 }),
            400,
        ),
        (
            json!({ "collection": "app", "target": "journey", "to_step": 1, "iterations": 2 }),
            400,
        ),
        (json!({ "collection": "app", "to_step": 1 }), 400),
    ] {
        let (status, doc) = api.call("POST", "/runs", Some(body.clone())).await;
        assert_eq!(status, want, "{body} → {doc}");
    }
}
