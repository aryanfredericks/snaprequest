//! Server-Sent Events through the API (PLAN P10.2): live events and stopping over the
//! WebSocket, parsed events in results.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::config::{AuthConfig, AuthMode};
use snapreq_server::store::Retention;
use snapreq_server::{Config, Server};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

type Resp = hyper::Response<http_body_util::combinators::UnsyncBoxBody<Bytes, Infallible>>;

/// `/events?n=N` (`0`: endless), 10 ms apart.
async fn target() -> SocketAddr {
    async fn handle(req: hyper::Request<hyper::body::Incoming>) -> Result<Resp, Infallible> {
        let n: usize = req
            .uri()
            .query()
            .and_then(|q| q.strip_prefix("n="))
            .and_then(|n| n.parse().ok())
            .unwrap_or(3);
        let s = futures_util::stream::unfold(0usize, move |i| async move {
            if n != 0 && i >= n {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((
                Ok(Frame::data(Bytes::from(format!(
                    "id: {i}\ndata: tick {i}\n\n"
                )))),
                i + 1,
            ))
        });
        Ok(hyper::Response::builder()
            .header("content-type", "text/event-stream")
            .body(BodyExt::boxed_unsync(StreamBody::new(s.boxed())))
            .unwrap())
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tcp), service_fn(handle))
                    .await;
            });
        }
    });
    addr
}

#[tokio::test]
async fn events_arrive_live_and_streams_can_be_stopped() {
    let root = std::env::temp_dir().join(format!("snapreq-sse-api-{}", std::process::id()));
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
    tokio::spawn(server.run(std::future::pending()));
    let sse = target().await;

    // REST: a finished stream comes back with its events.
    let mut req = Request::new(
        http::Method::POST,
        format!("http://{addr}/api/v1/requests/execute"),
    );
    req.body = Body::Json {
        content:
            json!({ "request": { "method": "GET", "url": format!("http://{sse}/events?n=2") } })
                .to_string(),
    };
    let resp = Client::default().execute(&req).await.unwrap();
    let doc: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(
        doc["events"],
        json!([{ "event": "message", "data": "tick 0", "id": "0" }, { "event": "message", "data": "tick 1", "id": "1" }])
    );

    // WebSocket: live events, then a cancel ends the endless stream with what arrived.
    let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/api/v1/ws"))
        .await
        .unwrap();
    let job = json!({ "type": "execute", "id": 7, "request": { "method": "GET", "url": format!("http://{sse}/events?n=0") } });
    ws.send(Message::Text(job.to_string().into()))
        .await
        .unwrap();
    let mut live = 0;
    let result = loop {
        let v: Value = match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => serde_json::from_str(&t).unwrap(),
            _ => continue,
        };
        assert_eq!(v["id"], 7);
        match v["type"].as_str().unwrap() {
            "event" => {
                assert_eq!(v["event"]["data"], format!("tick {live}"));
                assert!(v["event"]["at_ms"].is_number());
                live += 1;
                if live == 3 {
                    let cancel = json!({ "type": "cancel", "id": 7 });
                    ws.send(Message::Text(cancel.to_string().into()))
                        .await
                        .unwrap();
                }
            }
            "result" => break v["result"].clone(),
            "trace" => {}
            other => panic!("unexpected {other}: {v}"),
        }
    };
    assert!(live >= 3);
    assert_eq!(result["response"]["status"], 200);
    assert!(result["events"].as_array().unwrap().len() >= 3);
    // The connection keeps working for the next job.
    let job = json!({ "type": "execute", "id": 8, "request": { "method": "GET", "url": format!("http://{sse}/events?n=1") } });
    ws.send(Message::Text(job.to_string().into()))
        .await
        .unwrap();
    loop {
        if let Message::Text(t) = ws.next().await.unwrap().unwrap() {
            let v: Value = serde_json::from_str(&t).unwrap();
            if v["type"] == "result" {
                assert_eq!(v["id"], 8);
                assert_eq!(v["result"]["events"].as_array().unwrap().len(), 1);
                break;
            }
        }
    }
}
