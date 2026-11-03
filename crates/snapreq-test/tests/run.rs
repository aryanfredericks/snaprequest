//! Collection and folder runs, iterations and data rows.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use snapreq_collection::{Collection, Secrets, Variables};
use snapreq_core::Client;
use snapreq_test::{Plan, Unit, data, execute, request_keys};
use tokio::net::TcpListener;

/// Echoes the path; `/counter` returns how often it was called.
async fn start() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            let hits = hits.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let hits = hits.clone();
                    async move {
                        let body = if req.uri().path() == "/counter" {
                            (hits.fetch_add(1, Ordering::SeqCst) + 1).to_string()
                        } else {
                            req.uri().to_string()
                        };
                        Ok::<_, Infallible>(hyper::Response::new(Full::new(Bytes::from(body))))
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    addr
}

fn collection(addr: SocketAddr) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("snapreq-run-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in [
        ("snapreq.toml", format!("format = 1\nname = \"R\"\norder = [\"users\", \"other\"]\n[vars]\nbase = \"http://{addr}\"\nuser = \"default\"\n")),
        ("users/folder.toml", "order = [\"count\", \"greet\"]\n".into()),
        ("users/count.toml", "url = \"{{base}}/counter\"\n[capture]\nn = { body = true }\n".into()),
        ("users/greet.toml", "url = \"{{base}}/hello/{{user}}/{{n}}\"\n[assert.body]\nstarts_with = \"/hello/\"\nnot_contains = \"bob\"\n".into()),
        ("other.toml", "url = \"{{base}}/other\"\n".into()),
    ] {
        let p = dir.join(path);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }
    dir
}

#[tokio::test]
async fn iterations_and_data_rows() {
    let addr = start().await;
    let dir = collection(addr);
    let (c, _) = Collection::load(&dir).unwrap();
    let secrets = Secrets::default();

    assert_eq!(
        request_keys(&c, None).unwrap(),
        ["users/count", "users/greet", "other"]
    );
    assert_eq!(
        request_keys(&c, Some("users/")).unwrap(),
        ["users/count", "users/greet"]
    );
    assert!(
        request_keys(&c, Some("nope"))
            .unwrap_err()
            .contains("no requests in folder `nope`")
    );

    let mut plan = Plan {
        collection: &c,
        environment: None,
        secrets: &secrets,
        client: Client::default(),
        overrides: Variables::default(),
        hidden: Vec::new(),
        rows: Vec::new(),
        iterations: 2,
    };
    let unit = Unit::Requests(request_keys(&c, Some("users")).unwrap());
    let runs = execute(&plan, &unit, |_| {}).await;
    assert_eq!(runs.len(), 2);
    let bodies: Vec<String> = runs
        .iter()
        .map(|it| {
            it.requests()[1]
                .response()
                .unwrap()
                .text()
                .unwrap()
                .to_owned()
        })
        .collect();
    // Each iteration captures its own `n`.
    assert_eq!(bodies, ["/hello/default/1", "/hello/default/2"]);
    assert!(runs.iter().all(|it| it.passed()));

    // One iteration per data row; rows beat collection variables, --var beats rows.
    plan.rows = data::from_csv("user\nalice\nbob\n").unwrap();
    let runs = execute(&plan, &unit, |_| {}).await;
    let greet: Vec<(String, bool)> = runs
        .iter()
        .map(|it| {
            let r = it.requests()[1];
            (r.response().unwrap().text().unwrap().to_owned(), r.passed())
        })
        .collect();
    assert_eq!(
        greet,
        [
            ("/hello/alice/3".into(), true),
            ("/hello/bob/4".into(), false)
        ]
    );
    assert_eq!(
        runs[1]
            .data
            .as_ref()
            .unwrap()
            .get("user")
            .unwrap()
            .value
            .as_deref(),
        Some("bob")
    );

    plan.overrides.set("user", "cli");
    let runs = execute(&plan, &unit, |_| {}).await;
    assert!(runs.iter().all(|it| {
        it.requests()[1]
            .response()
            .unwrap()
            .text()
            .unwrap()
            .starts_with("/hello/cli/")
    }));
}
