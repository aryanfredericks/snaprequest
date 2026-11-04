//! `snapreq serve` end to end.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

fn snapreq() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_snapreq"));
    c.env_remove("SNAPREQ_LOG")
        .env_remove("SNAPREQ_BIND")
        .env_remove("SNAPREQ_CONFIG")
        .env_remove("SNAPREQ_ALLOW_REMOTE")
        .env_remove("SNAPREQ_AUTH")
        .env_remove("SNAPREQ_DATA_DIR");
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
fn remote_binds_without_accounts_are_refused_without_allow_remote() {
    let out = snapreq()
        .args([
            "serve",
            "--bind",
            "0.0.0.0:0",
            "--auth",
            "none",
            "--data-dir",
        ])
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

/// Runs `snapreq user ARGS --data-dir DATA` with `stdin` as input.
fn user(data: &std::path::Path, args: &[&str], stdin: &str) -> (i32, String, String) {
    use std::io::Write as _;
    let mut child = snapreq()
        .arg("user")
        .args(args)
        .arg("--data-dir")
        .arg(data)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        out.status.code().unwrap(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[tokio::test]
async fn accounts_are_managed_from_the_command_line() {
    let data = std::env::temp_dir().join(format!("snapreq-cli-users-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&data);
    let (code, _, err) = user(&data, &["add", "ana"], "ana-password\n");
    assert_eq!(code, 0, "{err}");
    assert!(err.contains("created user `ana`"), "{err}");
    assert!(
        err.contains("--auth accounts"),
        "a loopback server has no login: {err}"
    );
    let (code, _, err) = user(&data, &["add", "ANA"], "other-password\n");
    assert_eq!(code, 3);
    assert!(err.contains("already exists"), "{err}");
    let (code, _, err) = user(&data, &["add", "bo"], "short\n");
    assert_eq!(code, 3);
    assert!(err.contains("at least 8 characters"), "{err}");
    assert_eq!(user(&data, &["add", "bo"], "bo-password\n").0, 0);
    assert_eq!(user(&data, &["list"], "").1, "ana\nbo\n");
    assert_eq!(user(&data, &["passwd", "ana"], "new-password\n").0, 0);
    assert_eq!(user(&data, &["passwd", "zed"], "new-password\n").0, 3);
    assert_eq!(user(&data, &["remove", "bo"], "").0, 0);
    assert_eq!(user(&data, &["remove", "bo"], "").0, 3);
    assert_eq!(user(&data, &["list"], "").1, "ana\n");
    let audit = snapreq()
        .args(["audit", "--data-dir"])
        .arg(&data)
        .output()
        .unwrap();
    let log = String::from_utf8_lossy(&audit.stdout);
    for line in ["user.created", "user.password_set", "user.removed"] {
        assert!(log.contains(line), "{log}");
    }
    assert!(
        !log.contains("new-password") && !log.contains("ana-password"),
        "no secrets: {log}"
    );

    // The server logs her in with the new password.
    let mut child = snapreq()
        .args([
            "serve",
            "--bind",
            "127.0.0.1:0",
            "--auth",
            "accounts",
            "--data-dir",
        ])
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
    let login = |password: &str| {
        let mut req =
            snapreq_core::Request::new(http::Method::POST, format!("{url}/api/v1/auth/login"))
                .header("X-SnapReq-CSRF", "1");
        req.body = snapreq_core::Body::Json {
            content: format!(r#"{{"username":"ana","password":"{password}"}}"#),
        };
        req
    };
    let client = snapreq_core::Client::default();
    assert_eq!(
        client.execute(&login("ana-password")).await.unwrap().status,
        401
    );
    assert_eq!(
        client.execute(&login("new-password")).await.unwrap().status,
        200
    );
    child.kill().unwrap();
    let _ = child.wait();
}

#[test]
fn backup_and_restore_from_the_command_line() {
    let root = std::env::temp_dir().join(format!("snapreq-cli-backup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let (old, new) = (root.join("old"), root.join("new"));
    assert_eq!(user(&old, &["add", "ana"], "ana-password\n").0, 0);
    std::fs::create_dir_all(old.join("workspace/shop")).unwrap();
    std::fs::write(
        old.join("workspace/shop/snapreq.toml"),
        "format = 1\nname = \"Shop\"\n",
    )
    .unwrap();
    let file = root.join("b.tar.gz");
    let out = snapreq()
        .args(["backup"])
        .arg(&file)
        .arg("--data-dir")
        .arg(&old)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(String::from_utf8_lossy(&out.stderr).contains("secret key is not included"));
    let out = snapreq()
        .args(["restore"])
        .arg(&file)
        .arg("--data-dir")
        .arg(&new)
        .output()
        .unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(user(&new, &["list"], "").1, "ana\n");
    assert!(new.join("workspace/shop/snapreq.toml").is_file());
    let again = snapreq()
        .args(["restore"])
        .arg(&file)
        .arg("--data-dir")
        .arg(&new)
        .output()
        .unwrap();
    assert_eq!(again.status.code(), Some(3));
    assert!(String::from_utf8_lossy(&again.stderr).contains("--force"));
}
