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
        "POST /api/v1/requests/execute",
        "GET /api/v1/history",
        "GET /api/v1/history/{id}",
        "GET /api/v1/runs",
        "POST /api/v1/runs",
        "GET /api/v1/runs/{id}",
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
    assert_eq!(names, ["url", "method", "status", "before", "limit"]);
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
