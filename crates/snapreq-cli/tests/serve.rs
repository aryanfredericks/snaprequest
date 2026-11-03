//! `snapreq serve` end to end.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn snapreq() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_snapreq"));
    c.env_remove("SNAPREQ_LOG")
        .env_remove("SNAPREQ_BIND")
        .env_remove("SNAPREQ_CONFIG")
        .env_remove("SNAPREQ_ALLOW_REMOTE");
    c
}

#[tokio::test]
async fn serve_starts_on_the_given_address_and_answers() {
    let data = std::env::temp_dir().join(format!("snapreq-cli-serve-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let mut child = snapreq()
        .args(["serve", "--bind", "127.0.0.1:0", "--data-dir"])
        .arg(&data)
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let url = line
        .strip_prefix("snapreq server listening on ")
        .and_then(|rest| rest.split_whitespace().next())
        .unwrap_or_else(|| panic!("unexpected first line: {line}"))
        .to_owned();
    assert!(
        line.contains(&format!("(data: {})", data.display())),
        "{line}"
    );

    let resp = snapreq_core::Client::default()
        .execute(&snapreq_core::Request::get(format!("{url}/healthz")))
        .await
        .unwrap();
    assert_eq!(resp.status, 200);
    assert!(data.is_dir());
    child.kill().unwrap();
    let _ = child.wait();
}

#[test]
fn remote_binds_are_refused_without_allow_remote() {
    let out = snapreq()
        .args(["serve", "--bind", "0.0.0.0:0", "--data-dir"])
        .arg(std::env::temp_dir())
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(3));
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("refusing to listen on 0.0.0.0:0"), "{err}");
    assert!(err.contains("--allow-remote"), "{err}");

    let bad = snapreq()
        .args(["serve", "--bind", "nope"])
        .output()
        .unwrap();
    assert_eq!(bad.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&bad.stderr).contains("invalid bind address `nope`"));
}
