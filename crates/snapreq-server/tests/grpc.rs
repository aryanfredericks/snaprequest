//! gRPC through the API (PLAN P10.4): describing services and calls over the WebSocket.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

#[path = "../../snapreq-grpc/examples/mock_grpc.rs"]
mod mock_grpc;

use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::config::{AuthConfig, AuthMode};
use snapreq_server::store::Retention;
use snapreq_server::{Config, Server};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn grpc_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(mock_grpc::serve(listener));
    addr
}

async fn call(base: &str, method: &str, path: &str, body: Value) -> (u16, Value) {
    let mut req = Request::new(
        http::Method::from_bytes(method.as_bytes()).unwrap(),
        format!("{base}/api/v1{path}"),
    );
    req.body = Body::Json {
        content: body.to_string(),
    };
    let resp = Client::default().execute(&req).await.unwrap();
    (
        resp.status,
        serde_json::from_slice(&resp.body).unwrap_or(Value::Null),
    )
}

async fn open(addr: SocketAddr, start: Value) -> Socket {
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/ws/grpc"))
        .await
        .unwrap();
    ws.send(Message::Text(start.to_string().into()))
        .await
        .unwrap();
    ws
}

/// Messages up to and including `status` or `error` (trace events left out).
async fn until_end(ws: &mut Socket) -> Vec<Value> {
    let mut out = Vec::new();
    while let Some(Ok(m)) = ws.next().await {
        let Message::Text(t) = m else { continue };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["type"] == "trace" {
            continue;
        }
        let end = v["type"] == "status" || v["type"] == "error";
        out.push(v);
        if end {
            break;
        }
    }
    out
}

#[tokio::test]
async fn services_are_described_and_calls_stream_over_the_websocket() {
    let root = std::env::temp_dir().join(format!("snapreq-grpc-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let server = Server::bind(Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: root.join("data"),
        workspace: root.join("workspace"),
        allow_remote: false,
        retention: Retention::default(),
        outbound: Default::default(),
        auth: AuthConfig {
            mode: AuthMode::None,
            ..AuthConfig::default()
        },
        oidc: None,
    })
    .await
    .unwrap();
    let addr = server.local_addr().unwrap();
    let base = format!("http://{addr}");
    tokio::spawn(server.run(std::future::pending()));
    let grpc = grpc_server().await;

    // A scratch request: services by reflection.
    let (status, doc) = call(
        &base,
        "POST",
        "/grpc/describe",
        json!({ "request": { "method": "GET", "url": format!("grpc://{grpc}") } }),
    )
    .await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["services"][0]["name"], "shop.v1.Orders");
    assert_eq!(doc["services"][0]["methods"][3]["client_streaming"], true);

    // A collection request with proto files and a secret token.
    call(&base, "POST", "/collections", json!({ "id": "g" })).await;
    call(&base, "PUT", "/collections/g", json!({ "format": 1, "name": "G", "vars": { "host": grpc.to_string(), "token": { "secret": true } } })).await;
    std::fs::create_dir_all(root.join("workspace/g/protos")).unwrap();
    std::fs::copy(mock_grpc::PROTO, root.join("workspace/g/protos/shop.proto")).unwrap();
    let (status, doc) = call(
        &base,
        "PUT",
        "/collections/g/requests/get",
        json!({
            "url": "grpc://{{host}}",
            "auth": { "type": "bearer", "token": "{{token}}" },
            "grpc": { "method": "shop.v1.Orders/Get", "protos": ["protos/shop.proto"], "message": "{\"id\": 5}" }
        }),
    )
    .await;
    assert_eq!(status, 200, "{doc}");
    call(
        &base,
        "PUT",
        "/collections/g/secrets/shared/token",
        json!({ "value": "grpc-secret-token" }),
    )
    .await;
    let (status, doc) = call(
        &base,
        "POST",
        "/grpc/describe",
        json!({ "collection": "g", "key": "get" }),
    )
    .await;
    assert_eq!(status, 200, "{doc}");

    let mut ws = open(
        addr,
        json!({ "type": "start", "collection": "g", "key": "get" }),
    )
    .await;
    let events = until_end(&mut ws).await;
    assert_eq!(events[0]["type"], "headers", "{events:?}");
    assert_eq!(events[1]["message"]["id"], "5");
    assert_eq!(
        events[1]["message"]["tags"],
        json!(["auth:Bearer <redacted>"])
    );
    assert_eq!(events[2]["status"]["name"], "OK");

    // Errors are statuses; bidirectional streaming with send and end.
    let mut ws = open(
        addr,
        json!({ "type": "start", "collection": "g", "key": "get", "messages": [{ "id": 404 }] }),
    )
    .await;
    let events = until_end(&mut ws).await;
    assert_eq!(events.last().unwrap()["status"]["name"], "NOT_FOUND");

    let mut ws = open(addr, json!({ "type": "start", "request": { "method": "GET", "url": format!("grpc://{grpc}") }, "method": "shop.v1.Orders/Chat", "messages": [{ "item": "a" }], "end": false })).await;
    let mut items = Vec::new();
    while items.is_empty() {
        let Some(Ok(Message::Text(t))) = ws.next().await else {
            panic!("closed")
        };
        let v: Value = serde_json::from_str(&t).unwrap();
        if v["type"] == "message" {
            items.push(v["message"]["item"].as_str().unwrap().to_owned());
        }
    }
    ws.send(Message::Text(
        json!({ "type": "send", "message": { "item": "b" } })
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    ws.send(Message::Text(json!({ "type": "end" }).to_string().into()))
        .await
        .unwrap();
    let rest = until_end(&mut ws).await;
    items.extend(
        rest.iter()
            .filter(|v| v["type"] == "message")
            .map(|v| v["message"]["item"].as_str().unwrap().to_owned()),
    );
    assert_eq!(items, ["A", "B"]);
    assert_eq!(rest.last().unwrap()["status"]["name"], "OK");

    // Proto files must be in the workspace.
    let (status, doc) = call(
        &base,
        "POST",
        "/grpc/describe",
        json!({ "collection": "g", "key": "get", "file": { "url": "grpc://{{host}}", "grpc": { "protos": [mock_grpc::PROTO] } } }),
    )
    .await;
    assert_eq!(status, 422, "{doc}");
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .contains("outside the workspace"),
        "{doc}"
    );
}
