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
async fn piped_output_is_the_body_only() {
    let addr = server().await;
    let out = snapreq(&["send", &format!("http://{addr}/hello"), "-q", "a=1"]).await;

    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(stdout(&out), "GET /hello?a=1 \n");
}

#[tokio::test]
async fn print_selects_parts() {
    let addr = server().await;
    let url = format!("http://{addr}/hello");
    let out = snapreq(&[
        "send",
        &url,
        "-p",
        "Hhb",
        "-H",
        "Authorization: Bearer abcdef123",
    ])
    .await;
    let text = stdout(&out);

    assert!(
        text.starts_with(&format!("GET {url} HTTP/1.1\nHost: {addr}\n")),
        "{text}"
    );
    // Request headers come from the trace, so credentials are redacted.
    assert!(
        text.contains("Authorization: Bearer <redacted>\n"),
        "{text}"
    );
    assert!(text.contains("\n\nHTTP/1.1 200 OK\n"), "{text}");
    assert!(text.ends_with("\n\nGET /hello \n"), "{text}");
    // No color codes when not on a terminal.
    assert!(!text.contains('\u{1b}'), "{text:?}");

    let colored = stdout(&snapreq(&["send", &url, "-p", "h", "--color", "always"]).await);
    assert!(colored.contains("\u{1b}["), "{colored:?}");

    let out = snapreq(&["send", &url, "-p", "x"]).await;
    assert_eq!(out.status.code(), Some(3));
}

#[tokio::test]
async fn json_bodies_are_pretty_printed() {
    let addr = server().await;
    let out = snapreq(&["send", &format!("http://{addr}/j"), "-d", r#"{"a":[1]}"#]).await;
    // The echo server replies as text/plain starting with the request line, so it is not JSON.
    assert!(stdout(&out).ends_with("{\"a\":[1]}\n"), "{}", stdout(&out));
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

#[tokio::test]
async fn verbose_trace_goes_to_stderr_and_keeps_stdout_clean() {
    let addr = server().await;
    let out = snapreq(&["send", &format!("http://{addr}/v"), "-v"]).await;
    let err = stderr(&out);

    assert_eq!(stdout(&out), "GET /v \n");
    assert!(err.contains(&format!("* connected to {addr}")), "{err}");
    assert!(err.contains("> GET http://"), "{err}");
    assert!(err.contains("< HTTP/1.1 200 OK"), "{err}");
    assert!(err.contains("* total "), "{err}");
    // -v has no timestamps or wire dump.
    assert!(!err.contains("wire: sent"), "{err}");

    let err = stderr(&snapreq(&["send", &format!("http://{addr}/v"), "-vv"]).await);
    assert!(err.contains("] * wire: sent"), "{err}");
    assert!(err.contains("] > GET /v HTTP/1.1"), "{err}");
}

#[tokio::test]
async fn failures_show_the_trace_with_v_and_a_hint_without() {
    let free = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let url = format!("http://{free}/");

    let err = stderr(&snapreq(&["send", &url]).await);
    assert!(
        err.ends_with("(run with -v to see what happened)\n"),
        "{err}"
    );

    let err = stderr(&snapreq(&["send", &url, "-v"]).await);
    assert!(err.contains("! TCP connect failed"), "{err}");
    assert!(!err.contains("run with -v"), "{err}");
}

fn json_doc(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("stdout is not JSON ({e}): {}", stdout(o)))
}

#[tokio::test]
async fn json_format_describes_the_whole_exchange() {
    let addr = server().await;
    let out = snapreq(&[
        "send",
        &format!("http://{addr}/j?key=hidden-key-1"),
        "--format",
        "json",
        "--secret",
        "hidden-key-1",
    ])
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    let doc = json_doc(&out);

    assert_eq!(doc["format"], 1);
    assert_eq!(doc["request"]["method"], "GET");
    assert_eq!(
        doc["request"]["url"],
        format!("http://{addr}/j?key=<redacted>")
    );
    assert_eq!(doc["response"]["status"], 200);
    assert_eq!(doc["response"]["body"]["encoding"], "utf8");
    assert!(doc["response"]["timings"]["total_ms"].as_f64().unwrap() > 0.0);
    assert!(doc["error"].is_null());
    // The server echoes the URL into the body, which is returned untouched; the secret
    // must not appear anywhere else in the document.
    let mut without_body = doc.clone();
    without_body["response"]["body"] = serde_json::Value::Null;
    assert!(
        !without_body.to_string().contains("hidden-key-1"),
        "{without_body}"
    );
    let trace = doc["response"]["trace"].to_string();
    assert!(!trace.contains("hidden-key-1"), "{trace}");
}

#[tokio::test]
async fn json_format_reports_failures_as_json() {
    let free = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap();
    let out = snapreq(&["send", &format!("http://{free}/"), "--format", "json"]).await;
    assert_eq!(out.status.code(), Some(5));
    let doc = json_doc(&out);
    assert!(doc["response"].is_null());
    assert_eq!(doc["error"]["kind"], "execution");
    assert_eq!(doc["error"]["phase"], "connect");
    assert_eq!(doc["error"]["timed_out"], false);
    assert!(doc["error"]["trace"].as_array().unwrap().len() >= 2);

    let out = snapreq(&["send", "http://a.test/", "-H", "bad", "--format", "json"]).await;
    assert_eq!(out.status.code(), Some(3));
    assert_eq!(json_doc(&out)["error"]["kind"], "invalid_request");
}

#[tokio::test]
async fn output_file_receives_the_body() {
    let addr = server().await;
    let dir = std::env::temp_dir().join(format!("snapreq-cli-o-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("body.txt");

    let out = snapreq(&[
        "send",
        &format!("http://{addr}/saved"),
        "-o",
        file.to_str().unwrap(),
        "-p",
        "hb",
    ])
    .await;
    assert_eq!(out.status.code(), Some(0), "{}", stderr(&out));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "GET /saved \n");
    // The body is not printed; the status and headers still are.
    assert!(stdout(&out).starts_with("HTTP/1.1 200 OK\n"));
    assert!(!stdout(&out).contains("GET /saved"));

    let json = json_doc(
        &snapreq(&[
            "send",
            &format!("http://{addr}/s"),
            "-o",
            file.to_str().unwrap(),
            "--format",
            "json",
        ])
        .await,
    );
    assert_eq!(json["body_file"], file.to_str().unwrap());
    assert_eq!(json["response"]["body"]["data"], "");

    let missing = dir.join("no-such-dir").join("x.txt");
    let out = snapreq(&[
        "send",
        &format!("http://{addr}/"),
        "-o",
        missing.to_str().unwrap(),
    ])
    .await;
    assert_eq!(out.status.code(), Some(3));
    assert!(stderr(&out).contains("does not exist"), "{}", stderr(&out));
}
