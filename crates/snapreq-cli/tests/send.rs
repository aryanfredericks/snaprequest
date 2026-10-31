//! End-to-end tests: run the real `snapreq` binary against a local server.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::process::Output;

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

/// `/status/<code>` replies with that status; anything else echoes method, path,
/// content type and body.
async fn handle(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    if let Some(code) = req.uri().path().strip_prefix("/status/") {
        let mut resp = hyper::Response::new(Full::new(Bytes::from("status page")));
        *resp.status_mut() = code.parse::<u16>().unwrap().try_into().unwrap();
        return Ok(resp);
    }
    let line = format!(
        "{} {} {}\n",
        req.method(),
        req.uri(),
        req.headers()
            .get("content-type")
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default()
    );
    let body = req.into_body().collect().await.unwrap().to_bytes();
    let mut out = line.into_bytes();
    out.extend_from_slice(&body);
    Ok(hyper::Response::new(Full::new(Bytes::from(out))))
}

async fn server() -> SocketAddr {
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

async fn snapreq(args: &[&str]) -> Output {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    tokio::task::spawn_blocking(move || {
        std::process::Command::new(env!("CARGO_BIN_EXE_snapreq"))
            .args(&args)
            .env_remove("SNAPREQ_LOG")
            .output()
            .unwrap()
    })
    .await
    .unwrap()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[tokio::test]
async fn get_prints_status_headers_and_body() {
    let addr = server().await;
    let out = snapreq(&["send", &format!("http://{addr}/hello"), "-q", "a=1"]).await;

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.starts_with("HTTP/1.1 200 OK\n"), "{text}");
    assert!(text.contains("\nGET /hello?a=1 \n"), "{text}");
}

#[tokio::test]
async fn json_body_implies_post() {
    let addr = server().await;
    let out = snapreq(&["send", &format!("http://{addr}/u"), "--json", r#"{"n":1}"#]).await;

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert!(
        stdout(&out).contains("POST /u application/json\n{\"n\":1}"),
        "{}",
        stdout(&out)
    );
}

#[tokio::test]
async fn fail_flag_turns_http_errors_into_exit_code_4() {
    let addr = server().await;
    let url = format!("http://{addr}/status/404");

    assert_eq!(snapreq(&["send", &url]).await.status.code(), Some(0));
    assert_eq!(
        snapreq(&["send", &url, "--fail"]).await.status.code(),
        Some(4)
    );
}

#[tokio::test]
async fn exit_codes_for_failures() {
    // Nothing listens on this port.
    let free = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let out = snapreq(&["send", &format!("http://{free}/")]).await;
    assert_eq!(out.status.code(), Some(5));
    assert!(
        stderr(&out).contains("could not connect"),
        "{}",
        stderr(&out)
    );

    let out = snapreq(&["send", "http://a.test/", "--file", "/no/such/file"]).await;
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("/no/such/file"), "{}", stderr(&out));

    let out = snapreq(&["send"]).await;
    assert_eq!(out.status.code(), Some(2));
}

#[tokio::test]
async fn timeouts_exit_with_code_6() {
    // Accepts connections but never answers.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            held.push(tcp);
        }
    });
    let out = snapreq(&["send", &format!("http://{addr}/"), "-t", "0.3"]).await;
    assert_eq!(out.status.code(), Some(6), "{}", stderr(&out));
    assert!(stderr(&out).contains("timed out"), "{}", stderr(&out));
}
