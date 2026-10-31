//! Integration tests for HTTP/1.1 execution against local test servers (no internet needed).
#![allow(clippy::unwrap_used)] // test-server helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use snapreq_core::model::{Body, KeyValue, MultipartPart, PartValue, Phase, TraceEvent};
use snapreq_core::{Client, Error, Request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Test server
// ---------------------------------------------------------------------------

struct TestServer {
    addr: SocketAddr,
    connections: Arc<AtomicUsize>,
}

impl TestServer {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.addr)
    }

    fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

/// Routes:
/// - `/echo`: replies with the request line, headers and body as text
/// - `/status/<code>`: empty reply with that status
/// - `/slow?ms=N`: waits N ms, then replies `done`
/// - `/close`: replies with `Connection: close`
/// - `/bytes`: binary body
/// - `/count`: replies with the number of body bytes received and their sum mod 2^32
async fn handle(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let path = req.uri().path().to_owned();
    let query = req.uri().query().unwrap_or_default().to_owned();
    let mut resp = hyper::Response::builder();

    let body: Bytes = if path == "/echo" {
        let mut out = format!("{} {}\n", req.method(), req.uri());
        for (name, value) in req.headers() {
            out.push_str(&format!(
                "{name}: {}\n",
                value.to_str().unwrap_or("<binary>")
            ));
        }
        out.push('\n');
        let body = req.into_body().collect().await.unwrap().to_bytes();
        out.push_str(&String::from_utf8_lossy(&body));
        out.into()
    } else if let Some(code) = path.strip_prefix("/status/") {
        resp = resp.status(code.parse::<u16>().unwrap());
        Bytes::new()
    } else if path == "/slow" {
        let ms: u64 = query.trim_start_matches("ms=").parse().unwrap();
        tokio::time::sleep(Duration::from_millis(ms)).await;
        "done".into()
    } else if path == "/close" {
        resp = resp.header("connection", "close");
        "bye".into()
    } else if path == "/count" {
        let body = req.into_body().collect().await.unwrap().to_bytes();
        let sum = body
            .iter()
            .fold(0u32, |acc, b| acc.wrapping_add(u32::from(*b)));
        format!("{} {sum}", body.len()).into()
    } else if path == "/bytes" {
        resp = resp.header("content-type", "application/octet-stream");
        Bytes::from_static(&[0, 159, 146, 150, 255])
    } else {
        resp = resp.status(404);
        Bytes::new()
    };
    Ok(resp.body(Full::new(body)).unwrap())
}

async fn start_server() -> TestServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = connections.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), service_fn(handle))
                    .await;
            });
        }
    });
    TestServer { addr, connections }
}

/// A raw server that answers only the first request on each connection, without
/// sending `Connection: close`.
///
/// - `HangUp::AfterResponse`: closes the socket right after answering, like a server
///   with a tiny keep-alive timeout.
/// - `HangUp::OnNextRequest`: keeps the socket open, then reads the next request and
///   closes without answering, the "stale keep-alive connection" race.
#[derive(Clone, Copy)]
enum HangUp {
    AfterResponse,
    OnNextRequest,
}

async fn read_request_head(stream: &mut tokio::net::TcpStream) -> bool {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut chunk).await {
            Ok(0) | Err(_) => return false,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    true
}

async fn start_closing_server(mode: HangUp) -> (SocketAddr, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = connections.clone();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                if !read_request_head(&mut stream).await {
                    return;
                }
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
                    .await
                    .unwrap();
                if let HangUp::OnNextRequest = mode {
                    read_request_head(&mut stream).await;
                }
                // Dropping the stream closes the connection.
            });
        }
    });
    (addr, connections)
}

fn event_names(trace: &[snapreq_core::model::TraceEntry]) -> Vec<&'static str> {
    trace
        .iter()
        .map(|e| match e.event {
            TraceEvent::DnsStart { .. } => "dns_start",
            TraceEvent::DnsEnd { .. } => "dns_end",
            TraceEvent::ConnectStart { .. } => "connect_start",
            TraceEvent::ConnectEnd { .. } => "connect_end",
            TraceEvent::ConnectionReused { .. } => "connection_reused",
            TraceEvent::TlsStart { .. } => "tls_start",
            TraceEvent::TlsEnd { .. } => "tls_end",
            TraceEvent::RequestHeaders { .. } => "request_headers",
            TraceEvent::RequestSent { .. } => "request_sent",
            TraceEvent::ResponseFirstByte { .. } => "response_first_byte",
            TraceEvent::ResponseHeaders { .. } => "response_headers",
            TraceEvent::ResponseComplete { .. } => "response_complete",
            TraceEvent::Redirect { .. } => "redirect",
            TraceEvent::Info { .. } => "info",
            TraceEvent::Error { .. } => "error",
            _ => "other",
        })
        .collect()
}

fn execution_error(err: Error) -> snapreq_core::ExecutionError {
    match err {
        Error::Execution(e) => *e,
        other => panic!("expected an execution error, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_with_query_and_headers() {
    let server = start_server().await;
    let req = Request::get(server.url("/echo?x=1"))
        .query("a", "b c")
        .header("X-Test", "yes");

    let resp = Client::default().execute(&req).await.unwrap();

    assert_eq!(resp.status, 200);
    assert_eq!(resp.version.as_str(), "HTTP/1.1");
    let text = resp.text().unwrap();
    assert!(text.starts_with("GET /echo?x=1&a=b+c\n"), "{text}");
    assert!(text.contains("x-test: yes\n"), "{text}");
    assert!(text.contains("user-agent: snapreq/"), "{text}");
    assert!(text.contains(&format!("host: {}\n", server.addr)), "{text}");
    assert_eq!(resp.remote_addr, Some(server.addr));
}

#[tokio::test]
async fn trace_and_timings_for_a_fresh_connection() {
    let server = start_server().await;
    let resp = Client::default()
        .execute(&Request::get(server.url("/echo")))
        .await
        .unwrap();

    // An IP literal needs no DNS lookup.
    assert_eq!(
        event_names(&resp.trace),
        [
            "connect_start",
            "connect_end",
            "request_headers",
            "request_sent",
            "response_first_byte",
            "response_headers",
            "response_complete",
        ]
    );
    assert!(
        resp.trace.windows(2).all(|w| w[0].at <= w[1].at),
        "trace out of order"
    );

    let t = &resp.timings;
    assert!(t.dns.is_none() && t.tls.is_none());
    assert!(t.connect.is_some() && t.send.is_some() && t.wait.is_some() && t.download.is_some());
    assert_eq!(t.total, resp.trace.last().unwrap().at);

    assert_eq!(resp.sizes.response_body, resp.body.len() as u64);
    assert!(resp.sizes.request_headers > 0);
    assert!(resp.sizes.response_headers > 0);
}

#[tokio::test]
async fn hostname_is_resolved_and_traced() {
    let server = start_server().await;
    let url = format!("http://localhost:{}/echo", server.addr.port());
    let resp = Client::default().execute(&Request::get(url)).await.unwrap();

    let names = event_names(&resp.trace);
    assert_eq!(&names[..2], ["dns_start", "dns_end"]);
    assert!(resp.timings.dns.is_some());
    assert!(
        resp.text()
            .unwrap()
            .contains(&format!("host: localhost:{}", server.addr.port()))
    );
}

#[tokio::test]
async fn post_json_body() {
    let server = start_server().await;
    let req = Request::new(http::Method::POST, server.url("/echo")).body(Body::Json {
        content: r#"{"name":"snap"}"#.into(),
    });
    let resp = Client::default().execute(&req).await.unwrap();
    let text = resp.text().unwrap();

    assert!(text.starts_with("POST /echo\n"), "{text}");
    assert!(text.contains("content-type: application/json\n"), "{text}");
    assert!(text.contains("content-length: 15\n"), "{text}");
    assert!(text.ends_with("\n\n{\"name\":\"snap\"}"), "{text}");
    assert_eq!(resp.sizes.request_body, 15);
}

#[tokio::test]
async fn form_body_and_custom_method() {
    let server = start_server().await;
    let req = Request::new(
        http::Method::from_bytes(b"PURGE").unwrap(),
        server.url("/echo"),
    )
    .body(Body::Form {
        fields: vec![KeyValue::new("a", "1 2"), KeyValue::new("b", "&")],
    });
    let text = Client::default().execute(&req).await.unwrap();
    let text = text.text().unwrap();

    assert!(text.starts_with("PURGE /echo\n"), "{text}");
    assert!(text.contains("content-type: application/x-www-form-urlencoded\n"));
    assert!(text.ends_with("\n\na=1+2&b=%26"), "{text}");
}

#[tokio::test]
async fn error_statuses_are_responses_not_errors() {
    let server = start_server().await;
    let resp = Client::default()
        .execute(&Request::get(server.url("/status/404")))
        .await
        .unwrap();
    assert_eq!(resp.status, 404);
    assert_eq!(resp.status_text(), Some("Not Found"));
    assert!(!resp.is_success());
}

#[tokio::test]
async fn binary_bodies_are_kept_exactly() {
    let server = start_server().await;
    let resp = Client::default()
        .execute(&Request::get(server.url("/bytes")))
        .await
        .unwrap();
    assert_eq!(&resp.body[..], &[0, 159, 146, 150, 255]);
    assert!(resp.text().is_none());
}

#[tokio::test]
async fn connections_are_reused() {
    let server = start_server().await;
    let client = Client::default();

    let first = client
        .execute(&Request::get(server.url("/echo")))
        .await
        .unwrap();
    let second = client
        .execute(&Request::get(server.url("/echo")))
        .await
        .unwrap();

    assert_eq!(server.connections(), 1);
    assert!(event_names(&first.trace).contains(&"connect_end"));
    let names = event_names(&second.trace);
    assert_eq!(names[0], "connection_reused");
    assert!(!names.contains(&"connect_start"));
    assert!(second.timings.connect.is_none());
}

#[tokio::test]
async fn connection_close_is_not_reused() {
    let server = start_server().await;
    let client = Client::default();
    for _ in 0..2 {
        let resp = client
            .execute(&Request::get(server.url("/close")))
            .await
            .unwrap();
        assert_eq!(resp.text(), Some("bye"));
        assert!(!event_names(&resp.trace).contains(&"connection_reused"));
    }
    assert_eq!(server.connections(), 2);
}

#[tokio::test]
async fn server_closing_an_idle_connection_does_not_break_the_next_request() {
    let (addr, _) = start_closing_server(HangUp::AfterResponse).await;
    let client = Client::default();
    for _ in 0..3 {
        let resp = client
            .execute(&Request::get(format!("http://{addr}/")))
            .await
            .unwrap();
        assert_eq!(resp.text(), Some("ok"));
    }
}

#[tokio::test]
async fn total_timeout_reports_the_wait_phase_with_a_partial_trace() {
    let server = start_server().await;
    let mut req = Request::get(server.url("/slow?ms=3000"));
    req.settings.timeout = Some(Duration::from_millis(200));

    let err = execution_error(Client::default().execute(&req).await.unwrap_err());

    assert!(err.timed_out);
    assert_eq!(err.phase, Phase::Wait);
    let names = event_names(&err.trace);
    assert!(names.contains(&"request_sent"), "{names:?}");
    assert_eq!(names.last(), Some(&"error"));
    assert!(err.message.contains("200 ms"), "{}", err.message);
}

#[tokio::test]
async fn connection_refused_is_a_connect_error() {
    // Reserve a port, then free it so nothing is listening.
    let addr = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();

    let err = execution_error(
        Client::default()
            .execute(&Request::get(format!("http://{addr}/")))
            .await
            .unwrap_err(),
    );

    assert_eq!(err.phase, Phase::Connect);
    assert!(!err.timed_out);
    assert!(err.message.contains("could not connect"), "{}", err.message);
    assert_eq!(event_names(&err.trace), ["connect_start", "info", "error"]);
}

#[tokio::test]
async fn unknown_host_is_a_dns_error() {
    let err = execution_error(
        Client::default()
            .execute(&Request::get("http://does-not-exist.invalid/"))
            .await
            .unwrap_err(),
    );
    assert_eq!(err.phase, Phase::Dns);
    assert_eq!(event_names(&err.trace), ["dns_start", "error"]);
}

#[tokio::test]
async fn response_serializes_to_json() {
    let server = start_server().await;
    let resp = Client::default()
        .execute(&Request::get(server.url("/echo")))
        .await
        .unwrap();
    let json = serde_json::to_value(&resp).unwrap();
    assert_eq!(json["status"], 200);
    assert_eq!(json["version"], "HTTP/1.1");
    assert_eq!(json["body"]["encoding"], "utf8");
    assert!(json["timings"]["total_ms"].as_f64().unwrap() > 0.0);
    assert_eq!(json["trace"][0]["event"], "connect_start");
}

#[tokio::test]
async fn stale_pooled_connection_is_retried_on_a_new_one() {
    let (addr, connections) = start_closing_server(HangUp::OnNextRequest).await;
    let client = Client::default();
    let url = format!("http://{addr}/");

    let first = client.execute(&Request::get(&url)).await.unwrap();
    assert_eq!(first.text(), Some("ok"));

    let second = client.execute(&Request::get(&url)).await.unwrap();
    assert_eq!(second.text(), Some("ok"));
    assert_eq!(connections.load(Ordering::SeqCst), 2);

    let names = event_names(&second.trace);
    assert_eq!(
        names,
        [
            // Attempt on the pooled connection: sent, but the server hung up.
            "connection_reused",
            "request_headers",
            "request_sent",
            "info",
            // Retry on a fresh connection.
            "connect_start",
            "connect_end",
            "request_headers",
            "request_sent",
            "response_first_byte",
            "response_headers",
            "response_complete",
        ]
    );
    // Timings describe the attempt that succeeded.
    assert!(second.timings.connect.is_some());
}

fn temp_file(name: &str, contents: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("snapreq-http1-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn header_line<'a>(echo: &'a str, name: &str) -> &'a str {
    echo.lines()
        .find_map(|l| l.strip_prefix(&format!("{name}: ")))
        .unwrap_or_else(|| panic!("no `{name}` header in:\n{echo}"))
}

#[tokio::test]
async fn multipart_upload_with_text_and_file_parts() {
    let server = start_server().await;
    let path = temp_file("report.csv", b"id,name\n1,snap\n");
    let req = Request::new(http::Method::POST, server.url("/echo")).body(Body::Multipart {
        parts: vec![
            MultipartPart {
                name: "kind".into(),
                value: PartValue::Text {
                    value: "csv".into(),
                },
                enabled: true,
            },
            MultipartPart {
                name: "upload".into(),
                value: PartValue::File {
                    path,
                    filename: None,
                    content_type: None,
                },
                enabled: true,
            },
        ],
    });

    let resp = Client::default().execute(&req).await.unwrap();
    let echo = resp.text().unwrap();

    let ct = header_line(echo, "content-type");
    let boundary = ct
        .strip_prefix("multipart/form-data; boundary=")
        .unwrap_or_else(|| panic!("bad content-type `{ct}`"));
    let (_, body) = echo.split_once("\n\n").unwrap();
    assert_eq!(header_line(echo, "content-length"), body.len().to_string());
    assert_eq!(
        body,
        format!(
            "--{boundary}\r\nContent-Disposition: form-data; name=\"kind\"\r\n\r\ncsv\r\n\
             --{boundary}\r\nContent-Disposition: form-data; name=\"upload\"; filename=\"report.csv\"\r\n\
             Content-Type: text/csv\r\n\r\nid,name\n1,snap\n\r\n--{boundary}--\r\n"
        )
    );
    assert_eq!(resp.sizes.request_body, body.len() as u64);
}

#[tokio::test]
async fn large_file_body_is_streamed_completely() {
    let server = start_server().await;
    let data: Vec<u8> = (0..5_000_000u32).map(|i| (i % 253) as u8).collect();
    let sum = data
        .iter()
        .fold(0u32, |acc, b| acc.wrapping_add(u32::from(*b)));
    let path = temp_file("large.bin", &data);

    let req = Request::new(http::Method::PUT, server.url("/count")).body(Body::File {
        path,
        content_type: None,
    });
    let resp = Client::default().execute(&req).await.unwrap();

    assert_eq!(resp.text().unwrap(), format!("{} {sum}", data.len()));
    assert_eq!(resp.sizes.request_body, data.len() as u64);
    let sent = resp
        .trace
        .iter()
        .find_map(|e| match e.event {
            TraceEvent::RequestSent { body_bytes, .. } => Some(body_bytes),
            _ => None,
        })
        .unwrap();
    assert_eq!(sent, data.len() as u64);
}

#[tokio::test]
async fn file_body_content_type_is_guessed() {
    let server = start_server().await;
    let path = temp_file("payload.json", br#"{"a":1}"#);
    let req = Request::new(http::Method::POST, server.url("/echo")).body(Body::File {
        path,
        content_type: None,
    });
    let resp = Client::default().execute(&req).await.unwrap();
    let echo = resp.text().unwrap();
    assert_eq!(header_line(echo, "content-type"), "application/json");
    assert!(echo.ends_with("\n\n{\"a\":1}"), "{echo}");
}

#[tokio::test]
async fn missing_upload_file_fails_before_connecting() {
    let server = start_server().await;
    let req = Request::new(http::Method::POST, server.url("/echo")).body(Body::File {
        path: "no/such/file.bin".into(),
        content_type: None,
    });
    let err = Client::default().execute(&req).await.unwrap_err();
    assert!(matches!(err, Error::InvalidRequest(_)), "{err:?}");
    assert!(err.to_string().contains("no/such/file.bin"), "{err}");
    assert_eq!(server.connections(), 0);
}
