//! Importing through the API (PLAN P9.1): a Postman collection with an environment becomes a
//! collection of the caller's workspace, with credentials sealed and editors only.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::config::{AuthConfig, AuthMode};
use snapreq_server::store::Retention;
use snapreq_server::{Config, Server};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../snapreq-interop/tests/fixtures")
            .join(name),
    )
    .unwrap()
}

async fn call(
    base: &str,
    token: &str,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = Request::new(
        http::Method::from_bytes(method.as_bytes()).unwrap(),
        format!("{base}/api/v1{path}"),
    )
    .header("Authorization", format!("Bearer {token}"));
    if let Some(b) = body {
        req.body = Body::Json {
            content: b.to_string(),
        };
    }
    let resp = Client::default().execute(&req).await.unwrap();
    (
        resp.status,
        serde_json::from_slice(&resp.body).unwrap_or(Value::Null),
    )
}

async fn start() -> (String, snapreq_server::store::Store, PathBuf) {
    let root = std::env::temp_dir().join(format!("snapreq-import-api-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: root.join("data"),
        workspace: root.join("workspace"),
        allow_remote: false,
        retention: Retention::default(),
        outbound: Default::default(),
        auth: AuthConfig {
            mode: AuthMode::Accounts,
            ..AuthConfig::default()
        },
        oidc: None,
    };
    let server = Server::bind(config).await.unwrap();
    let base = format!("http://{}", server.local_addr().unwrap());
    let store = server.store().clone();
    tokio::spawn(server.run(std::future::pending()));
    (base, store, root.join("workspace"))
}

#[tokio::test]
async fn editors_import_collections_with_sealed_credentials() {
    let (base, store, workspace) = start().await;
    let ana = store.create_user("ana", "ana-password").await.unwrap();
    let (_, ana_token) = store.create_token(ana.id, "t", None).await.unwrap();
    let bo = store.create_user("bo", "bo-password").await.unwrap();
    let (_, bo_token) = store.create_token(bo.id, "t", None).await.unwrap();
    call(
        &base,
        &ana_token,
        "PUT",
        "/workspaces/default/members/bo",
        Some(json!({ "role": "viewer" })),
    )
    .await;

    let body = json!({
        "content": fixture("postman-shop.json"),
        "environments": [fixture("postman-dev.postman_environment.json")],
    });
    // Viewers cannot import.
    let (status, _) = call(
        &base,
        &bo_token,
        "POST",
        "/collections/import",
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, 403);

    let (status, doc) = call(
        &base,
        &ana_token,
        "POST",
        "/collections/import",
        Some(body.clone()),
    )
    .await;
    assert_eq!(status, 201, "{doc}");
    assert_eq!(doc["id"], "shop-api");
    assert_eq!(doc["format"], "postman");
    assert_eq!(doc["requests"], 7);
    assert_eq!(doc["environments"], json!(["dev-env"]));
    assert_eq!(doc["secrets"], 4);
    assert!(doc["warnings"].as_array().unwrap().len() >= 3);

    // The collection is there, and secret values are sealed on disk.
    let (status, c) = call(&base, &ana_token, "GET", "/collections/shop-api", None).await;
    assert_eq!(status, 200);
    assert_eq!(c["items"].as_array().map(Vec::len), Some(2), "{c}");
    let sealed = std::fs::read_to_string(workspace.join("shop-api/.secrets/dev-env.toml")).unwrap();
    assert!(sealed.contains("enc:v1:") && !sealed.contains("dev-token-value-123"));
    let (_, vars) = call(
        &base,
        &ana_token,
        "GET",
        "/collections/shop-api/variables?env=dev-env",
        None,
    )
    .await;
    let token = vars["variables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "token")
        .unwrap();
    assert_eq!(token["has_value"], true, "{vars}");
    assert!(!vars.to_string().contains("dev-token-value-123"));

    // Taken ids, unknown formats and unreadable files are errors.
    let (status, _) = call(&base, &ana_token, "POST", "/collections/import", Some(body)).await;
    assert_eq!(status, 409);
    let (status, doc) = call(
        &base,
        &ana_token,
        "POST",
        "/collections/import",
        Some(json!({ "content": "curl https://a.test", "format": "yaml" })),
    )
    .await;
    assert_eq!(status, 400);
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown format")
    );
    let (status, _) = call(
        &base,
        &ana_token,
        "POST",
        "/collections/import",
        Some(json!({ "content": "hello there" })),
    )
    .await;
    assert_eq!(status, 400);
    let (status, _) = call(
        &base,
        &ana_token,
        "POST",
        "/collections/import",
        Some(json!({ "content": "curl https://a.test", "id": "../evil" })),
    )
    .await;
    assert_eq!(status, 400);

    // A cURL command with a chosen id and name; larger than the usual 2 MiB body limit.
    let big = format!(
        "curl https://a.test/upload -H 'Content-Type: text/plain' --data-binary '{}'",
        "x".repeat(3 << 20)
    );
    let (status, doc) = call(
        &base,
        &ana_token,
        "POST",
        "/collections/import",
        Some(json!({ "content": big, "id": "scratch", "name": "Scratch" })),
    )
    .await;
    assert_eq!(status, 201, "{doc}");
    assert_eq!(doc["name"], "Scratch");

    let log = store.audit_log(None, None, 10).await.unwrap();
    assert!(
        log.iter().any(|e| e.action == "collection.imported"
            && e.target.as_deref() == Some("shop-api (postman)"))
    );
}
