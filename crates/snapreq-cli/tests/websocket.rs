//! End-to-end tests of `snapreq ws`.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns
#![allow(clippy::result_large_err)] // tungstenite's handshake callback type

use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::handshake::server::{
    ErrorResponse, Request as HsRequest, Response as HsResponse,
};

/// Echoes messages; sends `token: <X-Token header>` first.
async fn server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let mut token = String::new();
                let cb = |req: &HsRequest, resp: HsResponse| -> Result<HsResponse, ErrorResponse> {
                    token = req
                        .headers()
                        .get("x-token")
                        .and_then(|v| v.to_str().ok())
                        .unwrap_or("none")
                        .to_owned();
                    Ok(resp)
                };
                let Ok(mut ws) = tokio_tungstenite::accept_hdr_async(tcp, cb).await else {
                    return;
                };
                ws.send(Message::text(format!("token: {token}")))
                    .await
                    .unwrap();
                while let Some(Ok(m)) = ws.next().await {
                    if m.is_close() {
                        let _ = ws.close(None).await;
                        break;
                    }
                    if (m.is_text() || m.is_binary()) && ws.send(m).await.is_err() {
                        break;
                    }
                }
            });
        }
    });
    addr
}

async fn run(args: &[&str], stdin: Option<&str>) -> Output {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let stdin = stdin.map(str::to_owned);
    tokio::task::spawn_blocking(move || {
        let mut child = std::process::Command::new(env!("CARGO_BIN_EXE_snapreq"))
            .args(&args)
            .env_remove("SNAPREQ_LOG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        if let Some(input) = stdin {
            child
                .stdin
                .take()
                .unwrap()
                .write_all(input.as_bytes())
                .unwrap();
        }
        drop(child.stdin.take());
        child.wait_with_output().unwrap()
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

fn collection(name: &str, addr: SocketAddr) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snapreq-cli-ws-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("environments")).unwrap();
    std::fs::write(dir.join("snapreq.toml"), "format = 1\nname = \"ws\"\n").unwrap();
    std::fs::write(
        dir.join("environments/dev.toml"),
        format!("ws = \"ws://{addr}\"\nroom = \"lobby\"\n"),
    )
    .unwrap();
    std::fs::write(
        dir.join("chat.toml"),
        r#"url = "{{ws}}/chat"
[headers]
X-Token = "{{room}}-token"
[websocket]
[[websocket.messages]]
name = "join"
kind = "json"
content = '{"join": "{{room}}"}'
[[websocket.messages]]
name = "bin"
kind = "binary"
content = "AAEC"
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("plain.toml"),
        format!("url = \"http://{addr}/\"\n"),
    )
    .unwrap();
    dir
}

#[tokio::test]
async fn sends_messages_and_prints_replies() {
    let addr = server().await;
    let url = format!("ws://{addr}/echo");
    let o = run(
        &[
            "ws",
            &url,
            "--send",
            "hello",
            "--count",
            "2",
            "-H",
            "X-Token: abc",
        ],
        None,
    )
    .await;
    assert!(o.status.success(), "{}", err(&o));
    let text = out(&o);
    assert_eq!(text, "> hello\n< token: abc\n< hello\n", "{text}");
    assert!(
        err(&o).contains(&format!("connected to ws://{addr}/echo")),
        "{}",
        err(&o)
    );
}

#[tokio::test]
async fn stdin_lines_are_sent_and_json_output_is_one_object_per_line() {
    let addr = server().await;
    let url = format!("ws://{addr}/echo");
    let o = run(
        &["ws", &url, "--format", "json", "--wait", "0.5"],
        Some("one\ntwo\n"),
    )
    .await;
    assert!(o.status.success(), "{}", err(&o));
    let docs: Vec<serde_json::Value> = out(&o)
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect();
    assert_eq!(docs[0]["event"], "open");
    assert_eq!(docs[0]["status"], 101);
    let received: Vec<&str> = docs
        .iter()
        .filter(|d| d["direction"] == "received")
        .map(|d| d["data"].as_str().unwrap())
        .collect();
    assert_eq!(received, ["token: none", "one", "two"]);
    assert_eq!(docs.last().unwrap()["event"], "close");
}

#[tokio::test]
async fn saved_messages_of_a_collection_request_use_variables() {
    let addr = server().await;
    let dir = collection("saved", addr);
    let o = run(
        &[
            "ws",
            "-c",
            dir.to_str().unwrap(),
            "chat",
            "-e",
            "dev",
            "--message",
            "all",
            "--count",
            "3",
        ],
        None,
    )
    .await;
    assert!(o.status.success(), "{}", err(&o));
    assert_eq!(
        out(&o),
        "> {\"join\": \"lobby\"}\n> [binary 3 bytes] 00 01 02\n< token: lobby-token\n< {\"join\": \"lobby\"}\n< [binary 3 bytes] 00 01 02\n"
    );

    let o = run(
        &[
            "ws",
            "-c",
            dir.to_str().unwrap(),
            "chat",
            "-e",
            "dev",
            "--message",
            "nope",
        ],
        None,
    )
    .await;
    assert_eq!(o.status.code(), Some(3));
    assert!(
        err(&o).contains("no saved message `nope` in `chat` (saved: join, bin)"),
        "{}",
        err(&o)
    );

    // `run` leaves WebSocket requests out.
    let o = run(&["run", dir.to_str().unwrap(), "--color", "never"], None).await;
    let text = out(&o);
    assert!(!text.contains("chat"), "{text}");
    assert!(text.contains("plain"), "{text}");
}

#[tokio::test]
async fn a_refused_connection_is_a_network_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let o = run(&["ws", &format!("ws://{addr}/"), "--send", "x"], None).await;
    assert_eq!(o.status.code(), Some(5), "{}", err(&o));
    let o = run(&["ws", "ws://localhost/", "--json", "{}"], None).await;
    assert_eq!(o.status.code(), Some(3));
    assert!(err(&o).contains("no request body"), "{}", err(&o));
    let _ = Path::new(".");
}
