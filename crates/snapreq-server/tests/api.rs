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
        let root = std::env::temp_dir().join(format!("snapreq-api-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let config = Config {
            bind: "127.0.0.1:0".parse().unwrap(),
            data_dir: root.join("data"),
            workspace: root.join("workspace"),
            allow_remote: false,
            retention: Retention::default(),
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
