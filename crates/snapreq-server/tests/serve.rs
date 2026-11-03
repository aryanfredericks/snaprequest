//! The server starts, answers and shuts down gracefully.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use snapreq_core::{Client, Request};
use snapreq_server::{Config, Server};

#[tokio::test]
async fn health_and_version_then_graceful_shutdown() {
    let data_dir = std::env::temp_dir()
        .join(format!("snapreq-serve-{}", std::process::id()))
        .join("nested");
    let _ = std::fs::remove_dir_all(&data_dir);
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: data_dir.clone(),
        workspace: data_dir.join("workspace"),
        allow_remote: false,
        retention: snapreq_server::store::Retention::default(),
    };
    let server = Server::bind(config).await.unwrap();
    assert!(data_dir.is_dir(), "the data directory is created");
    assert!(
        data_dir.join("snapreq.db").is_file(),
        "the database is created"
    );
    assert_eq!(server.store().schema_version().await.unwrap(), 1);
    let addr = server.local_addr().unwrap();
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(server.run(async {
        let _ = stopped.await;
    }));

    let client = Client::default();
    let health = client
        .execute(&Request::get(format!("http://{addr}/healthz")))
        .await
        .unwrap();
    assert_eq!(health.status, 200);
    assert_eq!(health.text(), Some(r#"{"status":"ok"}"#));
    let version = client
        .execute(&Request::get(format!("http://{addr}/api/v1/version")))
        .await
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&version.body).unwrap();
    assert_eq!(doc["version"], snapreq_core::VERSION);
    assert_eq!(doc["api"], 1);
    assert_eq!(
        client
            .execute(&Request::get(format!("http://{addr}/nope")))
            .await
            .unwrap()
            .status,
        404
    );

    stop.send(()).unwrap();
    task.await.unwrap().unwrap();
    assert!(
        client
            .execute(&Request::get(format!("http://{addr}/healthz")))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn binding_a_busy_port_fails_clearly() {
    let taken = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let config = Config {
        bind: taken.local_addr().unwrap(),
        data_dir: std::env::temp_dir().join(format!("snapreq-serve-busy-{}", std::process::id())),
        workspace: std::env::temp_dir(),
        allow_remote: false,
        retention: snapreq_server::store::Retention::default(),
    };
    let err = Server::bind(config).await.unwrap_err().to_string();
    assert!(
        err.starts_with(&format!("cannot listen on {}", taken.local_addr().unwrap())),
        "{err}"
    );
}
