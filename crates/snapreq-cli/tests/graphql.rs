//! End-to-end tests of GraphQL requests and `snapreq introspect`.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::process::{Output, Stdio};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

const SCHEMA: &str =
    include_str!("../../snapreq-interop/tests/fixtures/graphql-introspection.json");

/// `/graphql`: the introspection fixture for introspection queries; otherwise echoes
/// `{ "method", "content_type", "body" }`. `/nope`: 404 text.
async fn server() -> SocketAddr {
    async fn handle(
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
        if req.uri().path() == "/nope" {
            return Ok(hyper::Response::builder()
                .status(404)
                .body(Full::new(Bytes::from("not here")))
                .unwrap());
        }
        let method = req.method().to_string();
        let ct = req
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let body = req.into_body().collect().await.unwrap().to_bytes();
        let doc: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
        let text = if doc["query"]
            .as_str()
            .is_some_and(|q| q.contains("__schema"))
        {
            SCHEMA.to_owned()
        } else {
            serde_json::json!({ "method": method, "content_type": ct, "body": doc }).to_string()
        };
        Ok(hyper::Response::builder()
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(text)))
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
async fn graphql_bodies_are_sent_as_json_with_post() {
    let addr = server().await;
    let url = format!("http://{addr}/graphql");
    let o = run(&[
        "send",
        &url,
        "--graphql",
        "query Me($x: Int) { me { id } }",
        "--variables",
        r#"{"x": 1}"#,
        "--operation",
        "Me",
        "-p",
        "b",
    ])
    .await;
    assert!(o.status.success(), "{}", err(&o));
    let doc: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(doc["method"], "POST");
    assert_eq!(doc["content_type"], "application/json");
    assert_eq!(
        doc["body"],
        serde_json::json!({ "query": "query Me($x: Int) { me { id } }", "variables": { "x": 1 }, "operationName": "Me" })
    );

    let o = run(&[
        "send",
        &url,
        "--graphql",
        "{ me { id } }",
        "--variables",
        "[1]",
    ])
    .await;
    assert_eq!(o.status.code(), Some(3));
    assert!(err(&o).contains("must be a JSON object"), "{}", err(&o));
}

#[tokio::test]
async fn collection_graphql_requests_use_files_and_variables() {
    let addr = server().await;
    let dir = std::env::temp_dir().join(format!("snapreq-cli-graphql-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("queries")).unwrap();
    std::fs::write(
        dir.join("snapreq.toml"),
        format!("format = 1\nname = \"gql\"\n[vars]\nbase = \"http://{addr}\"\nid = \"42\"\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("queries/user.graphql"),
        "query User($id: ID!) { user(id: $id) { name } }",
    )
    .unwrap();
    std::fs::write(
        dir.join("user.toml"),
        "url = \"{{base}}/graphql\"\n[body]\ntype = \"graphql\"\nquery_file = \"queries/user.graphql\"\nvariables = '{\"id\": \"{{id}}\"}'\n",
    )
    .unwrap();
    let o = run(&["send", "-c", dir.to_str().unwrap(), "user", "-p", "b"]).await;
    assert!(o.status.success(), "{}", err(&o));
    let doc: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(doc["method"], "POST");
    assert_eq!(doc["body"]["variables"]["id"], "42");
    assert!(
        doc["body"]["query"]
            .as_str()
            .unwrap()
            .contains("user(id: $id)")
    );

    // The listing says POST.
    let o = run(&["ls", dir.to_str().unwrap()]).await;
    assert!(out(&o).contains("POST"), "{}", out(&o));

    // Introspection uses the request's URL, headers and auth.
    let o = run(&["introspect", "-c", dir.to_str().unwrap(), "user"]).await;
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("user(id: ID!): User"), "{}", out(&o));
}

#[tokio::test]
async fn introspection_prints_sdl_or_json() {
    let addr = server().await;
    let url = format!("http://{addr}/graphql");
    let o = run(&["introspect", &url]).await;
    assert!(o.status.success(), "{}", err(&o));
    let sdl = out(&o);
    assert!(
        sdl.starts_with("type Query {\n  \"The signed-in user.\"\n  me: User\n"),
        "{sdl}"
    );
    assert!(sdl.contains("enum Role {\n  ADMIN\n  CUSTOMER\n}"), "{sdl}");
    assert!(!sdl.contains("scalar String"), "{sdl}");

    let o = run(&["introspect", &url, "--format", "json"]).await;
    let doc: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(doc["queryType"]["name"], "Query");

    let o = run(&["introspect", &format!("http://{addr}/nope")]).await;
    assert_eq!(o.status.code(), Some(4));
    assert!(err(&o).contains("not JSON"), "{}", err(&o));
}
