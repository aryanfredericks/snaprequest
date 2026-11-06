//! Streaming response bodies and Server-Sent Events (PLAN P10.2).
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::StreamBody;
use hyper::body::Frame;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use snapreq_core::model::TraceEvent;
use snapreq_core::sse::{self, TimedEvent};
use snapreq_core::{Client, Request, Streaming};
use tokio::net::TcpListener;

type Body = StreamBody<futures_util::stream::BoxStream<'static, Result<Frame<Bytes>, Infallible>>>;

/// `/events?n=N`: N events 20 ms apart, then the end (`n=0`: forever). `/plain`: text.
async fn server() -> SocketAddr {
    async fn handle(
        req: hyper::Request<hyper::body::Incoming>,
    ) -> Result<hyper::Response<Body>, Infallible> {
        use futures_util::StreamExt;
        if req.uri().path() == "/plain" {
            let s =
                futures_util::stream::once(async { Ok(Frame::data(Bytes::from("hello"))) }).boxed();
            return Ok(hyper::Response::new(StreamBody::new(s)));
        }
        let n: usize = req
            .uri()
            .query()
            .and_then(|q| q.strip_prefix("n="))
            .and_then(|n| n.parse().ok())
            .unwrap_or(3);
        let s = futures_util::stream::unfold(0usize, move |i| async move {
            if n != 0 && i >= n {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
            let chunk = format!("id: {i}\nevent: tick\ndata: {{\"i\": {i}}}\n\n");
            Some((Ok(Frame::data(Bytes::from(chunk))), i + 1))
        })
        .boxed();
        Ok(hyper::Response::builder()
            .header("content-type", "text/event-stream")
            .body(StreamBody::new(s))
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

fn collect() -> (Streaming, Arc<Mutex<Vec<TimedEvent>>>) {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = seen.clone();
    (
        Streaming::new(sse::observer(move |e| sink.lock().unwrap().push(e.clone()))),
        seen,
    )
}

#[tokio::test]
async fn events_are_observed_as_they_arrive() {
    let addr = server().await;
    let (stream, seen) = collect();
    let resp = Client::default()
        .execute_streaming(
            &Request::get(format!("http://{addr}/events?n=3")),
            None,
            Some(stream),
        )
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    assert_eq!(seen.len(), 3);
    assert_eq!(seen[2].event.event, "tick");
    assert_eq!(seen[2].event.id.as_deref(), Some("2"));
    assert_eq!(seen[2].event.data, "{\"i\": 2}");
    assert!(seen[2].at > seen[0].at);
    // The whole body is kept and parses to the same events.
    assert_eq!(sse::SseParser::parse_all(&resp.body).len(), 3);
}

#[tokio::test]
async fn an_endless_stream_can_be_stopped_and_keeps_what_arrived() {
    let addr = server().await;
    let (stream, seen) = collect();
    let stop = stream.stop.clone();
    let watcher = seen.clone();
    tokio::spawn(async move {
        while watcher.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        stop.cancel();
    });
    let resp = Client::default()
        .execute_streaming(
            &Request::get(format!("http://{addr}/events?n=0")),
            None,
            Some(stream),
        )
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    assert!(sse::SseParser::parse_all(&resp.body).len() >= 2);
    assert!(resp.trace.iter().any(|e| matches!(
        &e.event,
        TraceEvent::Info { message } if message.starts_with("stopped reading the body")
    )));
}

#[tokio::test]
async fn other_responses_are_not_parsed_and_bodies_can_be_capped() {
    let addr = server().await;
    let (mut stream, seen) = collect();
    stream.keep_limit = 2;
    let resp = Client::default()
        .execute_streaming(
            &Request::get(format!("http://{addr}/plain")),
            None,
            Some(stream),
        )
        .await
        .unwrap();
    assert!(seen.lock().unwrap().is_empty());
    assert_eq!(&resp.body[..], b"he");
    assert_eq!(resp.sizes.response_body, 5);
}
