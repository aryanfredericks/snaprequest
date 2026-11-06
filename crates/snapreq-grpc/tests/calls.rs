//! gRPC calls against the mock server: reflection, proto files, every kind of method.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

#[path = "../examples/mock_grpc.rs"]
mod mock_grpc;

use std::net::SocketAddr;

use serde_json::json;
use snapreq_core::model::{Auth, TraceEvent};
use snapreq_core::{Client, Request};
use snapreq_grpc::{CallEvent, call, connect, describe, find_method, message_from_json, reflect};
use tokio::net::TcpListener;

async fn server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(mock_grpc::serve(listener));
    addr
}

fn inputs(
    method: &snapreq_grpc::MethodDescriptor,
    jsons: &[&str],
) -> futures_util::stream::Iter<std::vec::IntoIter<snapreq_grpc::DynamicMessage>> {
    let msgs: Vec<_> = jsons
        .iter()
        .map(|j| message_from_json(&method.input(), j).unwrap())
        .collect();
    futures_util::stream::iter(msgs)
}

#[tokio::test]
async fn reflection_lists_services_and_unary_calls_carry_metadata() {
    let addr = server().await;
    let req = Request::get(format!("grpc://{addr}"))
        .header("X-User", "ana")
        .auth(Auth::Bearer {
            token: "grpc-s3cret".into(),
        });
    let channel = connect(&Client::default(), &req, None).await.unwrap();
    assert!(
        channel
            .trace
            .iter()
            .any(|e| matches!(e.event, TraceEvent::ConnectEnd { .. }))
    );
    let pool = reflect(&channel).await.unwrap();
    let services = describe(&pool);
    assert_eq!(services.len(), 1, "{services:?}");
    let s = &services[0];
    assert_eq!(s.name, "shop.v1.Orders");
    let kinds: Vec<(&str, &str)> = s
        .methods
        .iter()
        .map(|m| (m.name.as_str(), m.kind()))
        .collect();
    assert_eq!(
        kinds,
        [
            ("Get", "unary"),
            ("Watch", "server streaming"),
            ("Upload", "client streaming"),
            ("Chat", "bidirectional streaming")
        ]
    );
    let upload = &s.methods[2];
    assert_eq!(
        upload.template,
        json!({ "id": "0", "item": "", "status": "PENDING", "tags": [""], "created": "1970-01-01T00:00:00Z" })
    );

    let get = find_method(&pool, "shop.v1.Orders/Get").unwrap();
    let mut events = Vec::new();
    let out = call(&channel, &get, inputs(&get, &[r#"{"id": 7}"#]), |e| {
        events.push(e)
    })
    .await
    .unwrap();
    assert!(out.status.ok(), "{:?}", out.status);
    assert_eq!(out.messages.len(), 1);
    assert_eq!(out.messages[0]["item"], "book for ana");
    assert_eq!(out.messages[0]["status"], "READY");
    assert_eq!(out.messages[0]["id"], "7");
    assert_eq!(out.messages[0]["tags"], json!(["auth:Bearer grpc-s3cret"]));
    assert_eq!(out.headers["x-served-by"], "mock-grpc");
    assert!(matches!(events[0], CallEvent::Headers(_)));
    assert!(channel.secrets.contains(&"grpc-s3cret".to_owned()));

    // Errors are statuses.
    let out = call(&channel, &get, inputs(&get, &[r#"{"id": 404}"#]), |_| {})
        .await
        .unwrap();
    assert_eq!(
        (out.status.code, out.status.name.as_str()),
        (5, "NOT_FOUND")
    );
    assert_eq!(out.status.message, "order 404 does not exist");
}

#[tokio::test]
async fn proto_files_and_streaming_methods() {
    let addr = server().await;
    let pool = snapreq_grpc::compile_protos(&[mock_grpc::PROTO.into()], &[]).unwrap();
    let channel = connect(&Client::default(), &Request::get(addr.to_string()), None)
        .await
        .unwrap();

    let watch = find_method(&pool, "shop.v1.Orders.Watch").unwrap();
    let mut seen = 0;
    let out = call(&channel, &watch, inputs(&watch, &[r#"{"id": 1}"#]), |e| {
        if matches!(e, CallEvent::Message { .. }) {
            seen += 1;
        }
    })
    .await
    .unwrap();
    assert_eq!(seen, 3);
    assert_eq!(out.messages[2]["status"], "READY");

    let upload = find_method(&pool, "shop.v1.Orders/Upload").unwrap();
    let out = call(
        &channel,
        &upload,
        inputs(&upload, &["{}", "{}", r#"{"item": "x"}"#]),
        |_| {},
    )
    .await
    .unwrap();
    assert_eq!(out.messages, [json!({ "count": 3 })]);

    let chat = find_method(&pool, "shop.v1.Orders/Chat").unwrap();
    let out = call(
        &channel,
        &chat,
        inputs(&chat, &[r#"{"item": "a"}"#, r#"{"item": "b"}"#]),
        |_| {},
    )
    .await
    .unwrap();
    let items: Vec<&str> = out
        .messages
        .iter()
        .map(|m| m["item"].as_str().unwrap())
        .collect();
    assert_eq!(items, ["A", "B"]);
}

#[tokio::test]
async fn mistakes_are_explained() {
    let addr = server().await;
    let pool = snapreq_grpc::compile_protos(&[mock_grpc::PROTO.into()], &[]).unwrap();
    let get = find_method(&pool, "shop.v1.Orders/Get").unwrap();
    let e = message_from_json(&get.input(), r#"{"idd": 1}"#)
        .unwrap_err()
        .to_string();
    assert!(e.contains("shop.v1.GetOrder"), "{e}");
    let e = find_method(&pool, "shop.v1.Orders/Nope")
        .unwrap_err()
        .to_string();
    assert!(e.contains("methods: Get, Watch, Upload, Chat"), "{e}");
    let e = find_method(&pool, "shop.Other/Get")
        .unwrap_err()
        .to_string();
    assert!(e.contains("services: shop.v1.Orders"), "{e}");
    let e = snapreq_grpc::compile_protos(&["/nope/missing.proto".into()], &[])
        .unwrap_err()
        .to_string();
    assert!(e.contains("missing.proto"), "{e}");

    // Nothing listens: a connect error with the trace.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let closed = listener.local_addr().unwrap();
    drop(listener);
    let err = connect(
        &Client::default(),
        &Request::get(format!("grpc://{closed}")),
        None,
    )
    .await
    .unwrap_err();
    assert!(
        matches!(
            err,
            snapreq_grpc::Error::Core(snapreq_core::Error::Execution(_))
        ),
        "{err}"
    );
    let _ = addr;
}
