//! WebSocket client against a local tungstenite server.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns
#![allow(clippy::result_large_err)] // tungstenite's handshake callback type

use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use futures_util::{SinkExt, StreamExt};
use snapreq_core::client::ws::{WsMessage, WsOptions};
use snapreq_core::model::{Auth, TraceEvent};
use snapreq_core::{Client, ClientConfig, ConnectPolicy, Error, Request};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HsRequest, Response as HsResponse,
};

/// `/echo` echoes every message; the first message it sends is the `Authorization` header
/// it saw. It picks the subprotocol `chat` when offered. Other paths are refused with 403.
async fn server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut auth = String::new();
                let callback =
                    |req: &HsRequest, mut resp: HsResponse| -> Result<HsResponse, ErrorResponse> {
                        if req.uri().path() != "/echo" {
                            let mut e = ErrorResponse::new(Some("go away".into()));
                            *e.status_mut() = http::StatusCode::FORBIDDEN;
                            return Err(e);
                        }
                        auth = req
                            .headers()
                            .get("authorization")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or("none")
                            .to_owned();
                        let offered = req
                            .headers()
                            .get("sec-websocket-protocol")
                            .and_then(|v| v.to_str().ok())
                            .unwrap_or_default();
                        if offered.split(',').any(|p| p.trim() == "chat") {
                            resp.headers_mut()
                                .insert("sec-websocket-protocol", "chat".parse().unwrap());
                        }
                        Ok(resp)
                    };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, callback).await else {
                    return;
                };
                ws.send(Message::text(format!("auth: {auth}")))
                    .await
                    .unwrap();
                while let Some(Ok(m)) = ws.next().await {
                    match m {
                        Message::Text(_) | Message::Binary(_) => {
                            if ws.send(m).await.is_err() {
                                break;
                            }
                        }
                        Message::Close(_) => break,
                        _ => {}
                    }
                }
            });
        }
    });
    addr
}

#[tokio::test]
async fn sends_and_receives_messages() {
    let addr = server().await;
    let req = Request::get(format!("ws://{addr}/echo")).auth(Auth::Bearer {
        token: "s3cret-token".into(),
    });
    let options = WsOptions {
        subprotocols: vec!["v2".into(), "chat".into()],
    };
    let ws = Client::default()
        .websocket(&req, &options, None)
        .await
        .unwrap();
    assert_eq!(ws.handshake.status, 101);
    assert_eq!(ws.protocol.as_deref(), Some("chat"));
    assert_eq!(ws.handshake.url, format!("ws://{addr}/echo"));
    // The handshake is traced like any request, with credentials hidden.
    let trace = &ws.handshake.trace;
    assert!(
        trace
            .iter()
            .any(|e| matches!(e.event, TraceEvent::ConnectEnd { .. }))
    );
    let sent = trace
        .iter()
        .find_map(|e| match &e.event {
            TraceEvent::RequestHeaders { headers, .. } => Some(headers.clone()),
            _ => None,
        })
        .unwrap();
    assert!(
        sent.iter()
            .any(|h| h.name == "Upgrade" && h.value == "websocket")
    );
    assert!(!format!("{trace:?}").contains("s3cret-token"));

    let (mut tx, mut rx) = ws.split();
    assert_eq!(
        rx.recv().await.unwrap().unwrap(),
        WsMessage::Text {
            data: "auth: Bearer s3cret-token".into()
        }
    );
    tx.send(WsMessage::Text {
        data: "hello".into(),
    })
    .await
    .unwrap();
    assert_eq!(
        rx.recv().await.unwrap().unwrap(),
        WsMessage::Text {
            data: "hello".into()
        }
    );
    let bin = WsMessage::Binary {
        data: Bytes::from_static(&[0, 159, 146, 150]),
    };
    tx.send(bin.clone()).await.unwrap();
    assert_eq!(rx.recv().await.unwrap().unwrap(), bin);
    tx.close(None, "bye").await.unwrap();
    // The server's close answer, then the end.
    while let Some(m) = rx.recv().await {
        if matches!(m.unwrap(), WsMessage::Close { .. }) {
            break;
        }
    }
}

#[tokio::test]
async fn a_refused_upgrade_is_an_error_with_the_status() {
    let addr = server().await;
    let req = Request::get(format!("ws://{addr}/nope"));
    let err = Client::default()
        .websocket(&req, &WsOptions::default(), None)
        .await
        .unwrap_err();
    let text = err.to_string();
    assert!(text.contains("answered 403"), "{text}");
    assert!(text.contains("go away"), "{text}");
    assert!(
        err.trace()
            .unwrap()
            .iter()
            .any(|e| matches!(e.event, TraceEvent::ResponseHeaders { status: 403, .. }))
    );
}

#[tokio::test]
async fn bad_urls_are_rejected_before_connecting() {
    let err = Client::default()
        .websocket(&Request::get("ftp://x.test/"), &WsOptions::default(), None)
        .await
        .unwrap_err();
    assert!(matches!(err, Error::InvalidUrl { .. }), "{err}");
}

#[derive(Debug)]
struct DenyAll;

impl ConnectPolicy for DenyAll {
    fn check(&self, _host: &str, _ip: Option<std::net::IpAddr>) -> Result<(), String> {
        Err("not allowed here".into())
    }
}

#[tokio::test]
async fn the_outbound_policy_applies() {
    let addr = server().await;
    let client = Client::new(ClientConfig {
        connect_policy: Some(Arc::new(DenyAll)),
        ..ClientConfig::default()
    })
    .unwrap();
    let err = client
        .websocket(
            &Request::get(format!("ws://{addr}/echo")),
            &WsOptions::default(),
            None,
        )
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not allowed here"), "{err}");
}
