//! The OpenAPI description covers every route.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::collections::BTreeSet;

#[test]
fn every_route_is_described() {
    let doc = serde_json::to_value(snapreq_server::openapi()).unwrap();
    assert_eq!(
        doc["openapi"].as_str().unwrap().split('.').next(),
        Some("3")
    );
    let mut found = BTreeSet::new();
    for (path, item) in doc["paths"].as_object().unwrap() {
        for method in item.as_object().unwrap().keys() {
            found.insert(format!("{} {path}", method.to_uppercase()));
        }
    }
    let expected: BTreeSet<String> = [
        "GET /healthz",
        "GET /api/v1/version",
        "GET /api/v1/auth/status",
        "POST /api/v1/auth/login",
        "GET /api/v1/auth/oidc/start",
        "GET /api/v1/auth/oidc/callback",
        "POST /api/v1/auth/logout",
        "POST /api/v1/auth/password",
        "GET /api/v1/auth/tokens",
        "POST /api/v1/auth/tokens",
        "DELETE /api/v1/auth/tokens/{id}",
        "GET /api/v1/audit",
        "GET /api/v1/workspaces",
        "POST /api/v1/workspaces",
        "PUT /api/v1/workspaces/{ws}",
        "DELETE /api/v1/workspaces/{ws}",
        "GET /api/v1/workspaces/{ws}/members",
        "PUT /api/v1/workspaces/{ws}/members/{username}",
        "DELETE /api/v1/workspaces/{ws}/members/{username}",
        "POST /api/v1/requests/execute",
        "GET /api/v1/history",
        "GET /api/v1/history/{id}",
        "DELETE /api/v1/history/{id}",
        "DELETE /api/v1/history",
        "GET /api/v1/runs",
        "POST /api/v1/runs",
        "GET /api/v1/runs/{id}",
        "GET /api/v1/runs/{id}/report/{format}",
        "GET /api/v1/collections",
        "POST /api/v1/collections",
        "GET /api/v1/collections/{id}",
        "PUT /api/v1/collections/{id}",
        "DELETE /api/v1/collections/{id}",
        "GET /api/v1/collections/{id}/requests/{key}",
        "PUT /api/v1/collections/{id}/requests/{key}",
        "DELETE /api/v1/collections/{id}/requests/{key}",
        "GET /api/v1/collections/{id}/flows/{key}",
        "PUT /api/v1/collections/{id}/flows/{key}",
        "DELETE /api/v1/collections/{id}/flows/{key}",
        "GET /api/v1/collections/{id}/environments/{name}",
        "PUT /api/v1/collections/{id}/environments/{name}",
        "DELETE /api/v1/collections/{id}/environments/{name}",
        "GET /api/v1/collections/{id}/variables",
        "GET /api/v1/collections/{id}/folders/{key}",
        "PUT /api/v1/collections/{id}/folders/{key}",
        "DELETE /api/v1/collections/{id}/folders/{key}",
        "POST /api/v1/collections/{id}/move",
        "PUT /api/v1/collections/{id}/secrets/{scope}/{name}",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect();
    assert_eq!(found, expected);

    let schemas = doc["components"]["schemas"].as_object().unwrap();
    for name in [
        "ExecuteBody",
        "RunBody",
        "ErrorBody",
        "HistorySummary",
        "RunSummary",
        "HistoryList",
    ] {
        assert!(schemas.contains_key(name), "missing schema {name}");
    }
    let params = &doc["paths"]["/api/v1/history"]["get"]["parameters"];
    let names: Vec<&str> = params
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        names,
        [
            "url",
            "method",
            "status",
            "class",
            "failed",
            "collection",
            "before",
            "limit"
        ]
    );
}

#[tokio::test]
async fn the_document_is_served() {
    let root = std::env::temp_dir().join(format!("snapreq-openapi-{}", std::process::id()));
    let config = snapreq_server::Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: root.join("data"),
        workspace: root.join("workspace"),
        allow_remote: false,
        retention: snapreq_server::store::Retention::default(),
        outbound: Default::default(),
        auth: snapreq_server::config::AuthConfig::default(),
        oidc: None,
    };
    let server = snapreq_server::Server::bind(config).await.unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.run(std::future::pending()));
    let resp = snapreq_core::Client::default()
        .execute(&snapreq_core::Request::get(format!(
            "http://{addr}/api/v1/openapi.json"
        )))
        .await
        .unwrap();
    let doc: serde_json::Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(doc["info"]["title"], "SnapRequest API");
    assert_eq!(doc["info"]["version"], snapreq_core::VERSION);
}

/// `web/openapi.json` is what the web UI's typed client is generated from (`npm run gen:api`).
/// Regenerate it with `SNAPREQ_UPDATE_OPENAPI=1 cargo test -p snapreq-server --test openapi`.
#[test]
fn the_web_ui_copy_is_up_to_date() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/openapi.json");
    let current = format!(
        "{}\n",
        serde_json::to_string_pretty(&snapreq_server::openapi()).unwrap()
    );
    if std::env::var_os("SNAPREQ_UPDATE_OPENAPI").is_some() {
        std::fs::write(&path, &current).unwrap();
        return;
    }
    let committed = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        committed.replace("\r\n", "\n") == current,
        "web/openapi.json is out of date: run `SNAPREQ_UPDATE_OPENAPI=1 cargo test -p snapreq-server --test openapi`, \
         then `npm --prefix web run gen:api`"
    );
}
