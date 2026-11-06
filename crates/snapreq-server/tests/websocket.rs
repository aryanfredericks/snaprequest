//! WebSocket sessions relayed by the server (PLAN P10.1).
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns
#![allow(clippy::result_large_err)] // tungstenite's handshake callback type

use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::config::{AuthConfig, AuthMode};
use snapreq_server::store::Retention;
use snapreq_server::{Config, Server};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HsRequest, Response as HsResponse,
};

/// Echoes messages; first sends the `Authorization` header it saw.
async fn target() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut auth = String::new();
                let cb = |req: &HsRequest, resp: HsResponse| -> Result<HsResponse, ErrorResponse> {
                    auth = req
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("none")
                        .to_owned();
                    Ok(resp)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, cb).await else {
                    return;
                };
                ws.send(Message::text(format!("hello {auth}")))
                    .await
                    .unwrap();
                while let Some(Ok(m)) = ws.next().await {
                    if m.is_close() {
                        let _ = ws.close(None).await;
                        break;
                    }
                    if (m.is_text() || m.is_binary()) && ws.send(m).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

async fn call(base: &str, method: &str, path: &str, body: Value) -> u16 {
    let mut req = Request::new(
        http::Method::from_bytes(method.as_bytes()).unwrap(),
        format!("{base}/api/v1{path}"),
    );
    req.body = Body::Json {
        content: body.to_string(),
    };
    Client::default().execute(&req).await.unwrap().status
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn next(ws: &mut Socket) -> Value {
    loop {
        match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            _ => continue,
        }
    }
}

/// The next message of `kind`, skipping trace events.
async fn next_of(ws: &mut Socket, kind: &str) -> Value {
    loop {
        let v = next(ws).await;
        if v["type"] == kind {
            return v;
        }
        assert_eq!(v["type"], "trace", "expected {kind}, got {v}");
    }
}

async fn open(addr: SocketAddr) -> Socket {
    let req = format!("ws://{addr}/api/v1/ws/websocket")
        .into_client_request()
        .unwrap();
    tokio_tungstenite::connect_async(req).await.unwrap().0
}

async fn send(ws: &mut Socket, v: Value) {
    ws.send(Message::Text(v.to_string().into())).await.unwrap();
}

#[tokio::test]
async fn collection_websocket_requests_are_relayed_with_secrets_hidden() {
    let root = std::env::temp_dir().join(format!("snapreq-ws-api-{}", std::process::id()));
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
    let echo = target().await;

    call(&base, "POST", "/collections", json!({ "id": "chat" })).await;
    let status = call(
        &base,
        "PUT",
        "/collections/chat",
        json!({ "format": 1, "name": "Chat", "vars": { "ws": format!("ws://{echo}"), "token": { "secret": true } } }),
    )
    .await;
    assert_eq!(status, 200);
    let status = call(
        &base,
        "PUT",
        "/collections/chat/requests/room",
        json!({
            "url": "{{ws}}/room",
            "auth": { "type": "bearer", "token": "{{token}}" },
            "websocket": { "messages": [{ "name": "auth", "content": "token={{token}}" }] }
        }),
    )
    .await;
    assert_eq!(status, 200);
    call(
        &base,
        "PUT",
        "/collections/chat/secrets/shared/token",
        json!({ "value": "very-secret-token" }),
    )
    .await;

    let mut ws = open(addr).await;
    send(
        &mut ws,
        json!({ "type": "connect", "collection": "chat", "key": "room" }),
    )
    .await;
    let opened = next_of(&mut ws, "open").await;
    assert_eq!(opened["handshake"]["status"], 101);
    assert_eq!(opened["messages"], json!(["auth"]));
    let first = next_of(&mut ws, "message").await;
    assert_eq!(first["direction"], "received");
    assert_eq!(first["message"]["data"], "hello Bearer <redacted>");

    send(&mut ws, json!({ "type": "send", "name": "auth" })).await;
    let sent = next_of(&mut ws, "message").await;
    assert_eq!(sent["direction"], "sent");
    assert_eq!(sent["message"]["data"], "token=<redacted>");
    let echoed = next_of(&mut ws, "message").await;
    assert_eq!(echoed["direction"], "received");
    assert_eq!(echoed["message"]["data"], "token=<redacted>");

    send(
        &mut ws,
        json!({ "type": "send", "message": { "kind": "binary", "data": "AAE=" } }),
    )
    .await;
    assert_eq!(next_of(&mut ws, "message").await["message"]["data"], "AAE=");
    assert_eq!(
        next_of(&mut ws, "message").await["message"]["kind"],
        "binary"
    );

    send(&mut ws, json!({ "type": "send", "name": "nope" })).await;
    assert!(
        next_of(&mut ws, "error").await["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no saved message")
    );

    send(&mut ws, json!({ "type": "close" })).await;
    let closed = next_of(&mut ws, "closed").await;
    assert_eq!(closed["by"], "client");
    assert_eq!(closed["code"], 1000);

    // A scratch request; a refused target is an error with the trace.
    let mut ws = open(addr).await;
    send(&mut ws, json!({ "type": "connect", "request": { "method": "GET", "url": format!("ws://{echo}/x") } })).await;
    assert_eq!(next_of(&mut ws, "open").await["handshake"]["status"], 101);
    let mut ws = open(addr).await;
    send(&mut ws, json!({ "type": "connect", "request": { "method": "GET", "url": format!("http://{addr}/api/v1/health") } })).await;
    let e = next_of(&mut ws, "error").await;
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap()
            .contains("instead of 101"),
        "{e}"
    );
    assert!(e["error"]["trace"].is_array());

    let mut ws = open(addr).await;
    send(
        &mut ws,
        json!({ "type": "connect", "collection": "chat", "key": "missing" }),
    )
    .await;
    assert_eq!(next(&mut ws).await["type"], "error");
}
