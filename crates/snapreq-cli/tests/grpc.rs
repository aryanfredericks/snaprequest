//! End-to-end tests of `snapreq grpc` against the mock gRPC server.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

#[path = "../../snapreq-grpc/examples/mock_grpc.rs"]
mod mock_grpc;

use std::net::SocketAddr;
use std::process::{Output, Stdio};

use tokio::net::TcpListener;

async fn server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(mock_grpc::serve(listener));
    addr
}

async fn run(args: &[&str]) -> Output {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_snapreq"))
            .args(&args)
            .env_remove("SNAPREQ_LOG")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    })
    .await
    .unwrap()
}

fn out(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn err(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[tokio::test]
async fn lists_describes_and_calls_by_reflection() {
    let addr = server().await;
    let target = addr.to_string();
    let o = run(&["grpc", &target, "--list"]).await;
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(
        out(&o),
        "shop.v1.Orders\n  Get(GetOrder) returns (Order)\n  Watch(GetOrder) returns (stream Order)\n  Upload(stream Order) returns (Summary)\n  Chat(stream Order) returns (stream Order)\n"
    );
    let o = run(&["grpc", &target, "--describe", "shop.v1.Orders/Get"]).await;
    let doc: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(doc["kind"], "unary");
    assert_eq!(doc["template"], serde_json::json!({ "id": "0" }));

    let o = run(&[
        "grpc",
        &target,
        "shop.v1.Orders/Get",
        "-m",
        r#"{"id": 3}"#,
        "-H",
        "x-user: bo",
        "--bearer",
        "tok-s3cret",
    ])
    .await;
    assert!(o.status.success(), "{}", err(&o));
    let msg: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(msg["item"], "book for bo");
    // The token went out as metadata, and is hidden when it comes back.
    assert_eq!(msg["tags"], serde_json::json!(["auth:Bearer <redacted>"]));
    assert!(err(&o).contains("OK (1 message(s)"), "{}", err(&o));

    let o = run(&[
        "grpc",
        &target,
        "shop.v1.Orders/Get",
        "-m",
        r#"{"id": 404}"#,
    ])
    .await;
    assert_eq!(o.status.code(), Some(4));
    assert!(
        err(&o).contains("NOT_FOUND (5): order 404 does not exist"),
        "{}",
        err(&o)
    );
}

#[tokio::test]
async fn streams_with_proto_files_and_json_output() {
    let addr = server().await;
    let target = format!("grpc://{addr}");
    let o = run(&[
        "grpc",
        &target,
        "shop.v1.Orders/Upload",
        "--proto",
        mock_grpc::PROTO,
        "-m",
        "{}",
        "-m",
        "{}",
        "--format",
        "json",
    ])
    .await;
    assert!(o.status.success(), "{}", err(&o));
    let doc: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(doc["messages"], serde_json::json!([{ "count": 2 }]));
    assert_eq!(doc["status"]["name"], "OK");
    assert_eq!(doc["headers"]["x-served-by"], "mock-grpc");

    let o = run(&[
        "grpc",
        &target,
        "shop.v1.Orders/Watch",
        "-m",
        r#"{"id": 1}"#,
    ])
    .await;
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(out(&o).matches("\"id\": \"1\"").count(), 3, "{}", out(&o));

    let o = run(&[
        "grpc",
        &target,
        "shop.v1.Orders/Get",
        "-m",
        "{}",
        "-m",
        "{}",
    ])
    .await;
    assert_eq!(o.status.code(), Some(3));
    assert!(err(&o).contains("takes one message"), "{}", err(&o));
}

#[tokio::test]
async fn collection_grpc_requests_use_their_table_and_variables() {
    let addr = server().await;
    let dir = std::env::temp_dir().join(format!("snapreq-cli-grpc-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("protos")).unwrap();
    std::fs::copy(mock_grpc::PROTO, dir.join("protos/shop.proto")).unwrap();
    std::fs::write(
        dir.join("snapreq.toml"),
        format!("format = 1\nname = \"g\"\n[vars]\nhost = \"{addr}\"\nid = \"9\"\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("get.toml"),
        "url = \"grpc://{{host}}\"\n[headers]\nx-user = \"carla\"\n[grpc]\nmethod = \"shop.v1.Orders/Get\"\nprotos = [\"protos/shop.proto\"]\nmessage = '{\"id\": {{id}}}'\n",
    )
    .unwrap();
    let o = run(&["grpc", "-c", dir.to_str().unwrap(), "get"]).await;
    assert!(o.status.success(), "{}", err(&o));
    let msg: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(msg["id"], "9");
    assert_eq!(msg["item"], "book for carla");
    // `run` leaves gRPC requests out; `ls` lists them.
    let o = run(&["ls", dir.to_str().unwrap()]).await;
    assert!(out(&o).contains("get"), "{}", out(&o));
}
