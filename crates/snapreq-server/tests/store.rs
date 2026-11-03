//! History and run storage.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::json;
use snapreq_core::model::Auth;
use snapreq_core::{Client, Request};
use snapreq_server::store::{HistoryEntry, HistoryQuery, Retention, RunEntry, Store};
use tokio::net::TcpListener;

/// Echoes the request (target, headers, body); `/big` returns 1.5 MB; `/bin` binary.
async fn start() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let svc = service_fn(|req: hyper::Request<hyper::body::Incoming>| async move {
                    let body: Bytes = match req.uri().path() {
                        "/big" => Bytes::from(vec![b'x'; 1_500_000]),
                        "/bin" => Bytes::from_static(&[0, 159, 146, 150]),
                        _ => {
                            let mut out = format!("{}\n", req.uri());
                            for (n, v) in req.headers() {
                                out.push_str(&format!("{n}: {}\n", v.to_str().unwrap()));
                            }
                            let b = req.into_body().collect().await.unwrap().to_bytes();
                            out.push_str(&String::from_utf8_lossy(&b));
                            Bytes::from(out)
                        }
                    };
                    Ok::<_, Infallible>(
                        hyper::Response::builder()
                            .header("set-cookie", "sid=cookie-secret-1")
                            .body(Full::new(body))
                            .unwrap(),
                    )
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    addr
}

async fn exchange(store: &Store, req: &Request, source: Option<(&str, &str)>) -> i64 {
    let result = Client::default().execute(req).await;
    store
        .add_history(HistoryEntry::from_exchange(req, result.as_ref(), source))
        .await
        .unwrap()
}

#[tokio::test]
async fn history_is_stored_without_secrets() {
    let addr = start().await;
    let store = Store::in_memory().await.unwrap();
    assert_eq!(store.schema_version().await.unwrap(), 2);

    let mut req = Request::new(
        http::Method::POST,
        format!("http://{addr}/echo?key=query-secret-9"),
    )
    .header("X-Api-Key", "header-secret-2")
    .header("X-Plain", "visible")
    .body(snapreq_core::Body::Text {
        content: "body has var-secret-3".into(),
        content_type: None,
    })
    .auth(Auth::Bearer {
        token: "bearer-secret-4".into(),
    });
    req.settings.secrets = vec!["query-secret-9".into(), "var-secret-3".into()];
    let id = exchange(&store, &req, Some(("shop", "orders/create"))).await;

    let record = store.history_entry(id).await.unwrap().unwrap();
    let text = serde_json::to_string(&record).unwrap();
    for secret in [
        "query-secret-9",
        "header-secret-2",
        "var-secret-3",
        "bearer-secret-4",
        "cookie-secret-1",
    ] {
        assert!(!text.contains(secret), "{secret} stored: {text}");
    }
    assert_eq!(record.summary.method, "POST");
    assert_eq!(
        record.summary.url,
        format!("http://{addr}/echo?key=<redacted>")
    );
    assert_eq!(record.summary.status, Some(200));
    assert_eq!(record.summary.collection.as_deref(), Some("shop"));
    assert_eq!(record.summary.request_key.as_deref(), Some("orders/create"));
    assert_eq!(record.request["auth"], "bearer");
    assert_eq!(
        record.request["headers"][0],
        serde_json::json!({"name": "X-Api-Key", "value": "<redacted>"})
    );
    assert_eq!(record.request["headers"][1]["value"], "visible");
    let response = record.response.unwrap();
    // The server echoed the bearer token and the body; both are hidden.
    assert!(
        response["body"]["data"]
            .as_str()
            .unwrap()
            .contains("authorization: Bearer <redacted>")
    );
    assert!(response["trace"].as_array().unwrap().len() > 3);
    assert!(store.history_entry(id + 100).await.unwrap().is_none());
}

#[tokio::test]
async fn bodies_failures_and_queries() {
    let addr = start().await;
    let store = Store::in_memory().await.unwrap();
    let big = exchange(&store, &Request::get(format!("http://{addr}/big")), None).await;
    let r = store
        .history_entry(big)
        .await
        .unwrap()
        .unwrap()
        .response
        .unwrap();
    assert_eq!(r["body"]["truncated"], true);
    assert_eq!(
        r["body"]["data"].as_str().unwrap().len(),
        snapreq_server::store::MAX_STORED_BODY
    );

    let bin = exchange(&store, &Request::get(format!("http://{addr}/bin")), None).await;
    let r = store
        .history_entry(bin)
        .await
        .unwrap()
        .unwrap()
        .response
        .unwrap();
    assert_eq!(
        r["body"],
        serde_json::json!({"encoding": "base64", "data": "AJ+Slg==", "truncated": false})
    );

    let failed = exchange(&store, &Request::get("http://127.0.0.1:1/down"), None).await;
    let rec = store.history_entry(failed).await.unwrap().unwrap();
    assert!(
        rec.summary
            .error
            .as_deref()
            .unwrap()
            .contains("TCP connect failed"),
        "{:?}",
        rec.summary.error
    );
    assert_eq!(rec.summary.status, None);
    assert!(rec.response.unwrap()["trace"].is_array());

    exchange(
        &store,
        &Request::new(http::Method::DELETE, format!("http://{addr}/orders/1")),
        None,
    )
    .await;
    let all = store
        .history(HistoryQuery {
            limit: 50,
            ..HistoryQuery::default()
        })
        .await
        .unwrap();
    assert_eq!(
        all.iter().map(|h| h.id).collect::<Vec<_>>(),
        [4, 3, 2, 1],
        "newest first"
    );
    let q = |f: fn(&mut HistoryQuery)| {
        let mut q = HistoryQuery {
            limit: 50,
            ..HistoryQuery::default()
        };
        f(&mut q);
        q
    };
    let ids =
        |v: Vec<snapreq_server::store::HistorySummary>| v.iter().map(|h| h.id).collect::<Vec<_>>();
    assert_eq!(
        ids(store
            .history(q(|q| q.url_contains = Some("ORDERS".into())))
            .await
            .unwrap()),
        [4]
    );
    assert_eq!(
        ids(store
            .history(q(|q| q.method = Some("delete".into())))
            .await
            .unwrap()),
        [4]
    );
    assert_eq!(
        ids(store.history(q(|q| q.status = Some(200))).await.unwrap()),
        [4, 2, 1]
    );
    assert_eq!(
        ids(store.history(q(|q| q.before_id = Some(3))).await.unwrap()),
        [2, 1]
    );
    assert_eq!(
        ids(store
            .history(HistoryQuery {
                limit: 1,
                ..HistoryQuery::default()
            })
            .await
            .unwrap()),
        [4]
    );
}

#[tokio::test]
async fn runs_and_retention() {
    let store = Store::in_memory().await.unwrap();
    let run = |passed: bool| RunEntry {
        kind: "flow".into(),
        target_key: "orders/checkout".into(),
        name: "Checkout".into(),
        passed,
        report: serde_json::json!({ "format": 1, "passed": passed, "pad": "x".repeat(1000) }),
        junit: None,
        html: None,
    };
    let first = store.add_run(run(true)).await.unwrap();
    let second = store.add_run(run(false)).await.unwrap();
    let list = store.runs(10).await.unwrap();
    assert_eq!(
        list.iter().map(|r| (r.id, r.passed)).collect::<Vec<_>>(),
        [(second, false), (first, true)]
    );
    assert_eq!(
        store.run_report(first).await.unwrap().unwrap()["passed"],
        true
    );

    // Age: entries older than 30 days go.
    let old = chrono_days_ago(40);
    store.backdate("runs", first, old).await.unwrap();
    let pruned = store.prune(Retention::default()).await.unwrap();
    assert_eq!((pruned.history, pruned.runs), (0, 1));
    assert!(store.run_report(first).await.unwrap().is_none());

    // Size: the oldest entries go until everything fits.
    for _ in 0..5 {
        store.add_run(run(true)).await.unwrap();
    }
    let pruned = store
        .prune(Retention {
            max_age: None,
            max_bytes: Some(3500),
        })
        .await
        .unwrap();
    assert_eq!(pruned.runs, 3);
    assert_eq!(store.runs(10).await.unwrap().len(), 3);
    let none = store
        .prune(Retention {
            max_age: None,
            max_bytes: None,
        })
        .await
        .unwrap();
    assert_eq!((none.history, none.runs), (0, 0));
}

fn chrono_days_ago(days: u64) -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    i64::try_from((now - Duration::from_secs(days * 24 * 3600)).as_millis()).unwrap()
}

#[tokio::test]
async fn databases_are_migrated_once_and_newer_ones_refused() {
    let dir = std::env::temp_dir().join(format!("snapreq-store-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("snapreq.db");
    {
        let store = Store::open(&path).await.unwrap();
        store
            .add_run(RunEntry {
                kind: "collection".into(),
                target_key: String::new(),
                name: "x".into(),
                passed: true,
                report: serde_json::json!({}),
                junit: None,
                html: None,
            })
            .await
            .unwrap();
    }
    let again = Store::open(&path).await.unwrap();
    assert_eq!(again.schema_version().await.unwrap(), 2);
    assert_eq!(
        again.runs(10).await.unwrap().len(),
        1,
        "data survives reopening"
    );
    drop(again);

    let conn = rusqlite::Connection::open(&path).unwrap();
    conn.execute_batch("PRAGMA user_version = 99").unwrap();
    drop(conn);
    let err = Store::open(&path).await.unwrap_err().to_string();
    assert!(
        err.contains("created by a newer SnapRequest (schema 99"),
        "{err}"
    );
}

#[tokio::test]
async fn history_filters_by_class_failure_and_collection_and_deletes() {
    let store = Store::in_memory().await.unwrap();
    let add = |method: &str, status: Option<u16>, collection: Option<&str>| HistoryEntry {
        method: method.into(),
        url: "http://x/".into(),
        status,
        duration_ms: Some(1.0),
        error: status.is_none().then(|| "refused".into()),
        collection: collection.map(Into::into),
        request_key: collection.map(|_| "k".into()),
        request: json!({}),
        response: None,
    };
    for e in [
        add("GET", Some(200), Some("shop")),
        add("GET", Some(404), None),
        add("POST", Some(201), None),
        add("GET", None, Some("shop")),
    ] {
        store.add_history(e).await.unwrap();
    }
    let ids = |q: HistoryQuery| {
        let store = store.clone();
        async move {
            store
                .history(HistoryQuery { limit: 50, ..q })
                .await
                .unwrap()
                .iter()
                .map(|h| h.id)
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(
        ids(HistoryQuery {
            status_class: Some(2),
            ..Default::default()
        })
        .await,
        [3, 1]
    );
    assert_eq!(
        ids(HistoryQuery {
            failed: Some(true),
            ..Default::default()
        })
        .await,
        [4]
    );
    assert_eq!(
        ids(HistoryQuery {
            failed: Some(false),
            ..Default::default()
        })
        .await,
        [3, 2, 1]
    );
    assert_eq!(
        ids(HistoryQuery {
            collection: Some("shop".into()),
            ..Default::default()
        })
        .await,
        [4, 1]
    );
    assert_eq!(store.delete_history(Some(2)).await.unwrap(), 1);
    assert_eq!(store.delete_history(Some(2)).await.unwrap(), 0);
    assert_eq!(store.delete_history(None).await.unwrap(), 3);
    assert!(ids(HistoryQuery::default()).await.is_empty());
}

#[tokio::test]
async fn a_version_1_database_is_upgraded_and_keeps_its_runs() {
    let dir = std::env::temp_dir().join(format!("snapreq-store-upgrade-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("snapreq.db");
    {
        // The schema as the first release created it, with one run.
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch(
            "CREATE TABLE history (id INTEGER PRIMARY KEY, created_at INTEGER NOT NULL, method TEXT NOT NULL,
               url TEXT NOT NULL, status INTEGER, duration_ms REAL, error TEXT, collection TEXT, request_key TEXT,
               request_json TEXT NOT NULL, response_json TEXT, size_bytes INTEGER NOT NULL);
             CREATE INDEX history_created_at ON history (created_at);
             CREATE TABLE runs (id INTEGER PRIMARY KEY, created_at INTEGER NOT NULL, kind TEXT NOT NULL,
               target_key TEXT NOT NULL, name TEXT NOT NULL, passed INTEGER NOT NULL, report_json TEXT NOT NULL,
               size_bytes INTEGER NOT NULL);
             CREATE INDEX runs_created_at ON runs (created_at);
             INSERT INTO runs VALUES (1, 0, 'flow', 'f', 'F', 1, '{\"format\":1}', 12);
             PRAGMA user_version = 1;",
        )
        .unwrap();
    }
    let store = Store::open(&path).await.unwrap();
    assert_eq!(store.schema_version().await.unwrap(), 2);
    assert_eq!(
        store.run_report(1).await.unwrap(),
        Some(json!({ "format": 1 }))
    );
    use snapreq_server::store::ReportFormat;
    assert_eq!(
        store.run_rendering(1, ReportFormat::Html).await.unwrap(),
        None
    );
    let id = store
        .add_run(RunEntry {
            kind: "flow".into(),
            target_key: "f".into(),
            name: "F".into(),
            passed: true,
            report: json!({}),
            junit: Some("<testsuites/>".into()),
            html: Some("<!DOCTYPE html>".into()),
        })
        .await
        .unwrap();
    assert_eq!(
        store
            .run_rendering(id, ReportFormat::Junit)
            .await
            .unwrap()
            .as_deref(),
        Some("<testsuites/>")
    );
}
