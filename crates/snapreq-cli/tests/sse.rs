//! End-to-end tests of `snapreq sse`.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::process::{Output, Stdio};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, StreamBody};
use hyper::body::Frame;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

type Body = http_body_util::combinators::UnsyncBoxBody<Bytes, Infallible>;

/// `/events?n=N` (`0`: endless): `tick` events with ids, plus a `note` event first; it echoes
/// `Last-Event-ID` in the note. `/missing`: 404.
async fn server() -> SocketAddr {
    async fn handle(
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<Body>, Infallible> {
        use futures_util::StreamExt;
        if req.uri().path() == "/missing" {
            return Ok(hyper::Response::builder()
                .status(404)
                .body(http_body_util::Full::new(Bytes::from("no such stream")).boxed_unsync())
                .unwrap());
        }
        let n: usize = req
            .uri()
            .query()
            .and_then(|q| q.strip_prefix("n="))
            .and_then(|n| n.parse().ok())
            .unwrap_or(3);
        let last = req
            .headers()
            .get("last-event-id")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("none")
            .to_owned();
        let accept = req
            .headers()
            .get("accept")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("none")
            .to_owned();
        let first = futures_util::stream::once(async move {
            Ok(Frame::data(Bytes::from(format!(
                "event: note\ndata: last={last} accept={accept}\n\n"
            ))))
        });
        let ticks = futures_util::stream::unfold(0usize, move |i| async move {
            if n != 0 && i >= n {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
            Some((
                Ok(Frame::data(Bytes::from(format!(
                    "id: {i}\nevent: tick\ndata: {i}\n\n"
                )))),
                i + 1,
            ))
        });
        Ok(hyper::Response::builder()
            .header("content-type", "text/event-stream")
            .body(BodyExt::boxed_unsync(StreamBody::new(
                first.chain(ticks).boxed(),
            )))
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
async fn prints_events_until_the_server_ends_the_stream() {
    let addr = server().await;
    let o = run(&["sse", &format!("http://{addr}/events?n=2")]).await;
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(
        out(&o),
        "event: note\ndata: last=none accept=text/event-stream\n\nid: 0\nevent: tick\ndata: 0\n\nid: 1\nevent: tick\ndata: 1\n\n"
    );
    assert!(
        err(&o).contains("3 event(s); the server ended the stream"),
        "{}",
        err(&o)
    );
}

#[tokio::test]
async fn endless_streams_stop_at_a_limit_and_events_can_be_filtered() {
    let addr = server().await;
    let url = format!("http://{addr}/events?n=0");
    let o = run(&[
        "sse",
        &url,
        "--event",
        "tick",
        "--max-events",
        "3",
        "--format",
        "json",
        "--last-event-id",
        "41",
    ])
    .await;
    assert!(o.status.success(), "{}", err(&o));
    let docs: Vec<serde_json::Value> = out(&o)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(docs.len(), 4, "{docs:?}");
    assert_eq!(docs[0]["event"], "tick");
    assert_eq!(docs[2]["id"], "2");
    assert!(docs[0]["at_ms"].is_number());
    assert_eq!(docs[3]["end"], "max_events");
    assert_eq!(docs[3]["events"], 3);

    let o = run(&["sse", &url, "--duration", "0.2"]).await;
    assert!(o.status.success(), "{}", err(&o));
    assert!(out(&o).contains("data: last=none"));
    assert!(err(&o).contains("reached --duration"), "{}", err(&o));
}

#[tokio::test]
async fn errors_and_non_streams_are_reported() {
    let addr = server().await;
    let o = run(&["sse", &format!("http://{addr}/missing")]).await;
    assert_eq!(o.status.code(), Some(4));
    assert!(err(&o).contains("answered 404"), "{}", err(&o));
    assert!(out(&o).contains("no such stream"));
}
