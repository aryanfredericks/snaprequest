//! GraphQL through the API (PLAN P10.3): schemas by introspection, GraphQL request files.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::config::{AuthConfig, AuthMode};
use snapreq_server::store::Retention;
use snapreq_server::{Config, Server};
use tokio::net::TcpListener;

const SCHEMA: &str =
    include_str!("../../snapreq-interop/tests/fixtures/graphql-introspection.json");

/// Introspection needs `Authorization: Bearer gql-token`; other queries are echoed.
async fn target() -> SocketAddr {
    async fn handle(
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
        let auth = req
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = req.into_body().collect().await.unwrap().to_bytes();
        let doc: Value = serde_json::from_slice(&body).unwrap_or_default();
        let text = if doc["query"]
            .as_str()
            .is_some_and(|q| q.contains("__schema"))
        {
            if auth == "Bearer gql-token" {
                SCHEMA.to_owned()
            } else {
                json!({ "errors": [{ "message": "introspection needs a token" }] }).to_string()
            }
        } else {
            json!({ "data": { "echo": doc } }).to_string()
        };
        Ok(hyper::Response::new(Full::new(Bytes::from(text))))
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

async fn call(base: &str, method: &str, path: &str, body: Value) -> (u16, Value) {
    let mut req = Request::new(
        http::Method::from_bytes(method.as_bytes()).unwrap(),
        format!("{base}/api/v1{path}"),
    );
    req.body = Body::Json {
        content: body.to_string(),
    };
    let resp = Client::default().execute(&req).await.unwrap();
    (
        resp.status,
        serde_json::from_slice(&resp.body).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn schemas_come_from_introspection_with_the_requests_auth() {
    let root = std::env::temp_dir().join(format!("snapreq-graphql-api-{}", std::process::id()));
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
    let gql = target().await;

    call(&base, "POST", "/collections", json!({ "id": "api" })).await;
    call(&base, "PUT", "/collections/api", json!({ "format": 1, "name": "API", "vars": { "base": format!("http://{gql}"), "token": { "secret": true } } })).await;
    let (status, doc) = call(
        &base,
        "PUT",
        "/collections/api/requests/me",
        json!({
            "url": "{{base}}/graphql",
            "auth": { "type": "bearer", "token": "{{token}}" },
            "body": { "type": "graphql", "query": "query Me($id: ID!) { user(id: $id) { name } }", "variables": "{\"id\": \"7\"}" }
        }),
    )
    .await;
    assert_eq!(status, 200, "{doc}");
    call(
        &base,
        "PUT",
        "/collections/api/secrets/shared/token",
        json!({ "value": "gql-token" }),
    )
    .await;

    let (status, doc) = call(
        &base,
        "POST",
        "/requests/graphql-schema",
        json!({ "collection": "api", "key": "me" }),
    )
    .await;
    assert_eq!(status, 200, "{doc}");
    assert_eq!(doc["schema"]["queryType"]["name"], "Query");
    assert!(doc["sdl"].as_str().unwrap().contains("user(id: ID!): User"));

    // Without the token the server's GraphQL error is passed on.
    let (status, doc) = call(
        &base,
        "POST",
        "/requests/graphql-schema",
        json!({ "request": { "method": "POST", "url": format!("http://{gql}/graphql") } }),
    )
    .await;
    assert_eq!(status, 422);
    assert!(
        doc["error"]["message"]
            .as_str()
            .unwrap()
            .contains("introspection needs a token"),
        "{doc}"
    );

    // The GraphQL request itself sends its JSON document.
    let (status, doc) = call(
        &base,
        "POST",
        "/requests/execute",
        json!({ "collection": "api", "key": "me" }),
    )
    .await;
    assert_eq!(status, 200, "{doc}");
    // The echo is the response body, as JSON text inside the result.
    let echoed = doc["response"]["body"].to_string();
    assert!(
        echoed.contains(r#"\"variables\":{\"id\":\"7\"}"#),
        "{echoed}"
    );
    assert_eq!(doc["response"]["status"], 200);
}
