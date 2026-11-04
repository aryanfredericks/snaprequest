//! Export through the API (PLAN P9.2): requests as code without secret values, history as HAR.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::config::{AuthConfig, AuthMode};
use snapreq_server::store::Retention;
use snapreq_server::{Config, Server};

async fn call(base: &str, method: &str, path: &str, body: Option<Value>) -> (u16, Value, String) {
    let mut req = Request::new(
        http::Method::from_bytes(method.as_bytes()).unwrap(),
        format!("{base}/api/v1{path}"),
    );
    if let Some(b) = body {
        req.body = Body::Json {
            content: b.to_string(),
        };
    }
    let resp = Client::default().execute(&req).await.unwrap();
    let text = String::from_utf8_lossy(&resp.body).into_owned();
    let disposition = resp
        .header("content-disposition")
        .unwrap_or_default()
        .to_owned();
    (
        resp.status,
        serde_json::from_str(&text).unwrap_or(Value::Null),
        disposition,
    )
}

#[tokio::test]
async fn requests_export_as_code_and_history_as_har() {
    let root = std::env::temp_dir().join(format!("snapreq-export-api-{}", std::process::id()));
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
    let base = format!("http://{}", server.local_addr().unwrap());
    tokio::spawn(server.run(std::future::pending()));

    // A collection request using a secret.
    call(&base, "POST", "/collections", Some(json!({ "id": "api" }))).await;
    let (status, _, _) = call(
        &base,
        "PUT",
        "/collections/api",
        Some(json!({ "format": 1, "name": "API", "vars": { "base": "https://api.test", "token": { "secret": true } } })),
    )
    .await;
    assert_eq!(status, 200);
    call(
        &base,
        "PUT",
        "/collections/api/requests/me",
        Some(json!({ "url": "{{base}}/me", "auth": { "type": "bearer", "token": "{{token}}" } })),
    )
    .await;
    call(
        &base,
        "PUT",
        "/collections/api/secrets/shared/token",
        Some(json!({ "value": "very-secret-token" })),
    )
    .await;

    let (status, doc, _) = call(
        &base,
        "POST",
        "/requests/export",
        Some(json!({ "format": "curl", "collection": "api", "key": "me" })),
    )
    .await;
    assert_eq!(status, 200, "{doc}");
    let curl = doc["content"].as_str().unwrap();
    assert!(curl.contains("https://api.test/me"), "{curl}");
    assert!(curl.contains("Bearer ${SNAPREQ_SECRET_TOKEN}"), "{curl}");
    assert!(!curl.contains("very-secret-token"));

    // Unsaved edits and variables, as for execute; a value given for a secret is hidden too.
    let (_, doc, _) = call(
        &base,
        "POST",
        "/requests/export",
        Some(json!({
            "format": "python", "collection": "api", "key": "me",
            "file": { "url": "{{base}}/me?t={{token}}" },
            "vars": { "token": "override-secret" },
        })),
    )
    .await;
    let py = doc["content"].as_str().unwrap();
    assert!(py.contains("os.environ[\"SNAPREQ_SECRET_TOKEN\"]"), "{py}");
    assert!(!py.contains("override-secret"), "{py}");

    // A raw request; unknown formats and missing targets are errors.
    let (_, doc, _) = call(
        &base,
        "POST",
        "/requests/export",
        Some(json!({ "format": "go", "request": { "method": "GET", "url": "https://a.test/x" } })),
    )
    .await;
    assert!(
        doc["content"]
            .as_str()
            .unwrap()
            .contains("http.NewRequest(\"GET\", \"https://a.test/x\", nil)")
    );
    let (status, doc, _) = call(
        &base,
        "POST",
        "/requests/export",
        Some(json!({ "format": "cobol", "request": { "method": "GET", "url": "https://a.test" } })),
    )
    .await;
    assert_eq!(status, 400);
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown export format")
    );
    let (status, _, _) = call(
        &base,
        "POST",
        "/requests/export",
        Some(json!({ "format": "curl" })),
    )
    .await;
    assert_eq!(status, 400);

    // History as HAR.
    let (status, doc, _) = call(
        &base,
        "POST",
        "/requests/execute",
        Some(json!({ "request": { "method": "GET", "url": format!("{base}/healthz"), "headers": [{ "name": "Authorization", "value": "Bearer abc" }] } })),
    )
    .await;
    assert_eq!(status, 200);
    let id = doc["history_id"].as_i64().unwrap();
    let (status, har, disposition) = call(&base, "GET", &format!("/history/{id}/har"), None).await;
    assert_eq!(status, 200);
    assert!(
        disposition.contains(&format!("snapreq-{id}.har")),
        "{disposition}"
    );
    assert_eq!(har["log"]["version"], "1.2");
    let entry = &har["log"]["entries"][0];
    assert_eq!(entry["request"]["method"], "GET");
    assert_eq!(entry["response"]["status"], 200);
    assert!(entry["startedDateTime"].as_str().unwrap().starts_with("20"));
    assert!(!har.to_string().contains("Bearer abc"));
    let (status, _, _) = call(&base, "GET", "/history/999999/har", None).await;
    assert_eq!(status, 404);
}
