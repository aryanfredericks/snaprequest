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
/// - `/gzip`: gzip-encoded text if the request accepts gzip, otherwise plain text
/// - `/redirect/<n>`: 302 to `/redirect/<n-1>`; `/redirect/0` replies `done`
/// - `/status-to/<code>?to=<url>`: redirect with that status to `url`
/// - `/login`: sets `sid=s3cr3t` and redirects to `/echo`
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
    } else if let Some(n) = path.strip_prefix("/redirect/") {
        let n: u32 = n.parse().unwrap();
        if n == 0 {
            "done".into()
        } else {
            resp = resp
                .status(302)
                .header("location", format!("/redirect/{}", n - 1));
            Bytes::new()
        }
    } else if let Some(code) = path.strip_prefix("/status-to/") {
        let to = url::form_urlencoded::parse(query.as_bytes())
            .find(|(k, _)| k == "to")
            .map(|(_, v)| v.into_owned())
            .unwrap();
        resp = resp
            .status(code.parse::<u16>().unwrap())
            .header("location", to);
        "redirecting".into()
    } else if path == "/login" {
        resp = resp
            .status(302)
            .header("set-cookie", "sid=s3cr3t; Path=/; HttpOnly")
            .header("location", "/echo");
        Bytes::new()
    } else if path == "/gzip" {
        let text = "compressible ".repeat(100);
        let accepts_gzip = req
            .headers()
            .get("accept-encoding")
            .is_some_and(|v| v.to_str().unwrap().contains("gzip"));
        if accepts_gzip {
            use std::io::Write;
            let mut e = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
            e.write_all(text.as_bytes()).unwrap();
            resp = resp.header("content-encoding", "gzip");
            e.finish().unwrap().into()
        } else {
            text.into()
        }
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

#[tokio::test]
async fn gzip_responses_are_decoded() {
    let server = start_server().await;
    let resp = Client::default()
        .execute(&Request::get(server.url("/gzip")))
        .await
        .unwrap();

    assert_eq!(resp.text().unwrap(), "compressible ".repeat(100));
    assert_eq!(resp.header("content-encoding"), Some("gzip"));
    assert_eq!(resp.sizes.response_body_decoded, 1300);
    assert!(resp.sizes.response_body < 100, "{:?}", resp.sizes);
    let note = resp
        .trace
        .iter()
        .find_map(|e| match &e.event {
            TraceEvent::Info { message } => Some(message.clone()),
            _ => None,
        })
        .unwrap();
    assert!(note.starts_with("decoded gzip body:"), "{note}");
}

#[tokio::test]
async fn decompression_can_be_turned_off() {
    let server = start_server().await;
    let mut req = Request::get(server.url("/gzip"));
    req.settings.decompress = false;
    let resp = Client::default().execute(&req).await.unwrap();

    // No Accept-Encoding is sent, so the server answers uncompressed.
    assert_eq!(resp.header("content-encoding"), None);
    assert_eq!(resp.sizes.response_body, resp.sizes.response_body_decoded);
}

fn redirects(trace: &[snapreq_core::model::TraceEntry]) -> Vec<(u16, String, String)> {
    trace
        .iter()
        .filter_map(|e| match &e.event {
            TraceEvent::Redirect { status, from, to } => Some((*status, from.clone(), to.clone())),
            _ => None,
        })
        .collect()
}

fn status_to(server: &TestServer, code: u16, to: &str) -> String {
    let to: String = url::form_urlencoded::byte_serialize(to.as_bytes()).collect();
    server.url(&format!("/status-to/{code}?to={to}"))
}

#[tokio::test]
async fn redirect_chains_are_followed_on_one_connection() {
    let server = start_server().await;
    let resp = Client::default()
        .execute(&Request::get(server.url("/redirect/3")))
        .await
        .unwrap();

    assert_eq!(resp.text(), Some("done"));
    assert_eq!(resp.url, server.url("/redirect/0"));
    let hops = redirects(&resp.trace);
    assert_eq!(hops.len(), 3);
    assert_eq!(
        hops[0],
        (302, server.url("/redirect/3"), server.url("/redirect/2"))
    );
    assert_eq!(server.connections(), 1);
    // Timings describe the final hop; the total covers the whole chain.
    assert!(resp.timings.connect.is_none());
    assert_eq!(resp.timings.total, resp.trace.last().unwrap().at);
}

#[tokio::test]
async fn see_other_after_post_becomes_get() {
    let server = start_server().await;
    let req = Request::new(http::Method::POST, status_to(&server, 303, "/echo")).body(Body::Json {
        content: "{}".into(),
    });
    let resp = Client::default().execute(&req).await.unwrap();
    let echo = resp.text().unwrap();

    assert!(echo.starts_with("GET /echo\n"), "{echo}");
    assert!(!echo.contains("content-type"), "{echo}");
}

#[tokio::test]
async fn temporary_redirect_resends_method_and_body() {
    let server = start_server().await;
    let req = Request::new(http::Method::PUT, status_to(&server, 307, "/echo")).body(Body::Text {
        content: "payload".into(),
        content_type: None,
    });
    let resp = Client::default().execute(&req).await.unwrap();
    let echo = resp.text().unwrap();

    assert!(echo.starts_with("PUT /echo\n"), "{echo}");
    assert!(echo.ends_with("\n\npayload"), "{echo}");
}

#[tokio::test]
async fn cross_origin_redirect_drops_credentials() {
    let a = start_server().await;
    let b = start_server().await;
    let req = Request::get(status_to(&a, 302, &b.url("/echo")))
        .header("Authorization", "Bearer secret")
        .header("X-Keep", "yes");
    let resp = Client::default().execute(&req).await.unwrap();
    let echo = resp.text().unwrap();

    assert!(echo.contains("x-keep: yes"), "{echo}");
    assert!(
        !echo.to_ascii_lowercase().contains("authorization"),
        "{echo}"
    );
    assert!(echo.contains(&format!("host: {}", b.addr)), "{echo}");

    // Same origin keeps the header.
    let same = Request::get(status_to(&a, 302, "/echo")).header("Authorization", "Bearer secret");
    let echo = Client::default().execute(&same).await.unwrap();
    assert!(
        echo.text()
            .unwrap()
            .contains("authorization: Bearer secret")
    );
}

#[tokio::test]
async fn too_many_redirects_is_an_error_with_the_trace() {
    let server = start_server().await;
    let mut req = Request::get(server.url("/redirect/5"));
    req.settings.max_redirects = 2;
    let err = execution_error(Client::default().execute(&req).await.unwrap_err());

    assert_eq!(err.phase, Phase::Redirect);
    assert!(
        err.message.contains("stopped after 2 redirect(s)"),
        "{}",
        err.message
    );
    assert_eq!(redirects(&err.trace).len(), 2);
}

#[tokio::test]
async fn redirects_are_returned_when_following_is_off() {
    let server = start_server().await;
    let mut req = Request::get(server.url("/redirect/1"));
    req.settings.follow_redirects = false;
    let resp = Client::default().execute(&req).await.unwrap();

    assert_eq!(resp.status, 302);
    assert_eq!(resp.header("location"), Some("/redirect/0"));
    assert!(redirects(&resp.trace).is_empty());
}

#[tokio::test]
async fn redirect_to_unsupported_scheme_fails() {
    let server = start_server().await;
    let err = execution_error(
        Client::default()
            .execute(&Request::get(status_to(&server, 302, "ftp://files.test/x")))
            .await
            .unwrap_err(),
    );
    assert_eq!(err.phase, Phase::Redirect);
    assert!(err.message.contains("`ftp` URL"), "{}", err.message);
}

fn infos(trace: &[snapreq_core::model::TraceEntry]) -> Vec<String> {
    trace
        .iter()
        .filter_map(|e| match &e.event {
            TraceEvent::Info { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn cookies_set_during_a_redirect_are_sent_on_the_next_hop() {
    let server = start_server().await;
    let client = Client::default();
    let resp = client
        .execute(&Request::get(server.url("/login")))
        .await
        .unwrap();

    let echo = resp.text().unwrap();
    assert!(echo.starts_with("GET /echo\n"), "{echo}");
    assert!(echo.contains("cookie: sid=s3cr3t\n"), "{echo}");
    assert_eq!(
        infos(&resp.trace),
        [
            "stored cookie `sid`",
            "sending 1 cookie(s) from the jar: sid"
        ]
    );

    let jar = client.cookies();
    assert_eq!(jar.len(), 1);
    assert_eq!(
        (jar[0].name.as_str(), jar[0].value.as_str()),
        ("sid", "s3cr3t")
    );
    assert!(jar[0].http_only);

    // Later requests to the same server carry the cookie too.
    let later = client
        .execute(&Request::get(server.url("/echo")))
        .await
        .unwrap();
    assert!(later.text().unwrap().contains("cookie: sid=s3cr3t\n"));
}

#[tokio::test]
async fn user_cookie_header_is_merged_with_the_jar() {
    let server = start_server().await;
    let client = Client::default();
    client
        .execute(&Request::get(server.url("/login")))
        .await
        .unwrap();

    let req = Request::get(server.url("/echo")).header("Cookie", "theme=dark");
    let echo = client.execute(&req).await.unwrap();
    assert!(
        echo.text()
            .unwrap()
            .contains("cookie: theme=dark; sid=s3cr3t\n")
    );

    // A user value for the same name wins.
    let req = Request::get(server.url("/echo")).header("Cookie", "sid=mine");
    let echo = client.execute(&req).await.unwrap();
    assert!(echo.text().unwrap().contains("cookie: sid=mine\n"));
}

#[tokio::test]
async fn cookies_can_be_disabled_per_request_and_cleared() {
    let server = start_server().await;
    let client = Client::default();

    let mut no_jar = Request::get(server.url("/login"));
    no_jar.settings.cookies = false;
    let resp = client.execute(&no_jar).await.unwrap();
    assert!(!resp.text().unwrap().contains("cookie:"));
    assert!(client.cookies().is_empty());

    client
        .execute(&Request::get(server.url("/login")))
        .await
        .unwrap();
    assert_eq!(client.cookies().len(), 1);
    client.clear_cookies();
    let resp = client
        .execute(&Request::get(server.url("/echo")))
        .await
        .unwrap();
    assert!(!resp.text().unwrap().contains("cookie:"));
}

fn wire(trace: &[snapreq_core::model::TraceEntry]) -> (String, String, bool) {
    trace
        .iter()
        .find_map(|e| match &e.event {
            TraceEvent::Wire {
                sent,
                received,
                truncated,
            } => Some((
                String::from_utf8_lossy(sent).into_owned(),
                String::from_utf8_lossy(received).into_owned(),
                *truncated,
            )),
            _ => None,
        })
        .expect("no wire event in trace")
}

#[tokio::test]
async fn wire_capture_records_the_exact_exchange() {
    let server = start_server().await;
    let mut req = Request::new(http::Method::POST, server.url("/echo")).body(Body::Text {
        content: "ping".into(),
        content_type: None,
    });
    req.settings.capture_wire = true;
    let resp = Client::default().execute(&req).await.unwrap();

    let (sent, received, truncated) = wire(&resp.trace);
    assert!(
        sent.starts_with(&format!("POST /echo HTTP/1.1\r\nHost: {}\r\n", server.addr)),
        "{sent}"
    );
    assert!(sent.ends_with("\r\n\r\nping"), "{sent}");
    assert!(received.starts_with("HTTP/1.1 200 OK\r\n"), "{received}");
    assert!(received.ends_with(resp.text().unwrap()), "{received}");
    assert!(!truncated);
    assert_eq!(
        sent.len() as u64,
        resp.sizes.request_headers + resp.sizes.request_body
    );
}

#[tokio::test]
async fn secrets_and_sensitive_headers_are_redacted_only_in_the_trace() {
    let server = start_server().await;
    let mut req = Request::get(server.url("/echo?key=topsecret"))
        .header("Authorization", "Bearer abc.def.ghi")
        .header("X-Note", "contains topsecret too");
    req.settings.capture_wire = true;
    req.settings.secrets = vec!["topsecret".into()];
    let resp = Client::default().execute(&req).await.unwrap();

    // What was sent (echoed back by the server) is untouched.
    let echo = resp.text().unwrap();
    assert!(echo.contains("authorization: Bearer abc.def.ghi"), "{echo}");
    assert!(echo.starts_with("GET /echo?key=topsecret"), "{echo}");

    // The trace is redacted everywhere, including the wire capture.
    let trace = serde_json::to_string(&resp.trace).unwrap();
    assert!(!trace.contains("topsecret"), "{trace}");
    assert!(!trace.contains("abc.def.ghi"), "{trace}");
    assert!(trace.contains("Bearer <redacted>"));
    assert!(trace.contains("key=<redacted>"));
    let (sent, _, _) = wire(&resp.trace);
    assert!(
        sent.contains("Authorization: Bearer <redacted>\r\n"),
        "{sent}"
    );
}

#[tokio::test]
async fn set_cookie_is_redacted_in_the_trace_but_not_in_the_response() {
    let server = start_server().await;
    let mut req = Request::get(server.url("/login"));
    req.settings.follow_redirects = false;
    let resp = Client::default().execute(&req).await.unwrap();

    assert_eq!(
        resp.header("set-cookie"),
        Some("sid=s3cr3t; Path=/; HttpOnly")
    );
    let traced = resp
        .trace
        .iter()
        .find_map(|e| match &e.event {
            TraceEvent::ResponseHeaders { headers, .. } => headers
                .iter()
                .find(|h| h.name == "set-cookie")
                .map(|h| h.value.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(traced, "sid=<redacted>; Path=/; HttpOnly");
}

#[tokio::test]
async fn wire_capture_is_capped() {
    let server = start_server().await;
    let client = Client::new(snapreq_core::ClientConfig {
        wire_capture_limit: 16,
        ..snapreq_core::ClientConfig::default()
    })
    .unwrap();
    let mut req = Request::get(server.url("/echo"));
    req.settings.capture_wire = true;
    let resp = client.execute(&req).await.unwrap();

    let (sent, received, truncated) = wire(&resp.trace);
    assert!(truncated);
    assert_eq!(sent.len(), 16);
    assert_eq!(received.len(), 16);
}

#[tokio::test]
async fn failed_requests_keep_their_wire_capture() {
    let server = start_server().await;
    let mut req = Request::get(server.url("/slow?ms=3000"));
    req.settings.capture_wire = true;
    req.settings.timeout = Some(Duration::from_millis(200));
    let err = execution_error(Client::default().execute(&req).await.unwrap_err());

    let (sent, received, _) = wire(&err.trace);
    assert!(sent.starts_with("GET /slow?ms=3000 HTTP/1.1\r\n"), "{sent}");
    assert!(received.is_empty());
}
