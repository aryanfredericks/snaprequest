//! End-to-end tests of collection commands: `send` with collection requests, `ls`,
//! `new` and `secret set`.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::io::Write;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::{Output, Stdio};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;

/// Echoes the request line, headers (sorted, lowercase) and body.
async fn echo(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let mut out = format!("{} {}\n", req.method(), req.uri());
    let mut headers: Vec<String> = req
        .headers()
        .iter()
        .map(|(n, v)| format!("{n}: {}", v.to_str().unwrap()))
        .collect();
    headers.sort();
    for h in headers {
        out.push_str(&h);
        out.push('\n');
    }
    let body = req.into_body().collect().await.unwrap().to_bytes();
    out.push('\n');
    out.push_str(&String::from_utf8_lossy(&body));
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
                    .serve_connection(TokioIo::new(tcp), service_fn(echo))
                    .await;
            });
        }
    });
    addr
}

/// A private copy of the collection fixture.
fn collection(name: &str) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let target = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &target);
            } else {
                std::fs::copy(e.path(), target).unwrap();
            }
        }
    }
    let from =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../snapreq-collection/tests/fixtures/basic");
    let to = std::env::temp_dir().join(format!("snapreq-cli-coll-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&to);
    copy(&from, &to);
    to
}

async fn run(
    args: &[&str],
    cwd: Option<&Path>,
    stdin: Option<&str>,
    env: &[(&str, &str)],
) -> Output {
    let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
    let cwd = cwd.map(Path::to_path_buf);
    let stdin = stdin.map(str::to_owned);
    let env: Vec<(String, String)> = env
        .iter()
        .map(|(k, v)| ((*k).into(), (*v).into()))
        .collect();
    tokio::task::spawn_blocking(move || {
        let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_snapreq"));
        cmd.args(&args)
            .env_remove("SNAPREQ_LOG")
            .env_remove("SNAPREQ_SECRET_TOKEN")
            .envs(env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(dir) = cwd {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn().unwrap();
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

#[tokio::test]
async fn sending_a_request_file_applies_variables_and_inheritance() {
    let addr = server().await;
    let dir = collection("file");
    let file = dir.join("users/create-user.toml");
    let o = run(
        &[
            "send",
            file.to_str().unwrap(),
            "-e",
            "dev",
            "--var",
            &format!("base_url=http://{addr}"),
            "--var",
            "token=tok-from-cli",
        ],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let text = out(&o);
    assert!(text.starts_with("POST /users\n"), "{text}");
    // Inherited from the collection and the `users` folder, plus the request's own.
    assert!(text.contains("accept: application/json\n"), "{text}");
    assert!(text.contains("x-section: users\n"), "{text}");
    assert!(
        text.contains("authorization: Bearer tok-from-cli\n"),
        "{text}"
    );
    assert!(text.contains("\"name\": \"snap\""), "{text}");
}

#[tokio::test]
async fn request_keys_work_with_c_and_from_inside_the_collection() {
    let addr = server().await;
    let dir = collection("keys");
    let base = format!("base_url=http://{addr}");

    let o = run(
        &[
            "send",
            "-c",
            dir.to_str().unwrap(),
            "users/list-users",
            "--var",
            &base,
        ],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(
        out(&o).starts_with("GET /users?page=2&tag=a&tag=b\n"),
        "{}",
        out(&o)
    );

    let o = run(&["send", "health", "--var", &base], Some(&dir), None, &[]).await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(out(&o).starts_with("GET /health\n"), "{}", out(&o));
}

#[tokio::test]
async fn command_line_options_override_collection_files() {
    let addr = server().await;
    let dir = collection("override");
    let o = run(
        &[
            "send",
            "-c",
            dir.to_str().unwrap(),
            "health",
            "--var",
            &format!("base_url=http://{addr}"),
            "-H",
            "accept: text/plain",
            "-q",
            "x=1",
        ],
        None,
        None,
        &[],
    )
    .await;
    let text = out(&o);
    assert!(text.starts_with("GET /health?x=1\n"), "{text}");
    assert!(text.contains("accept: text/plain\n"), "{text}");
    assert!(!text.contains("application/json"), "{text}");
}

#[tokio::test]
async fn secrets_come_from_env_vars_and_secret_set() {
    let addr = server().await;
    let dir = collection("secrets");
    let args = [
        "send",
        "-c",
        dir.to_str().unwrap(),
        "users/create-user",
        "-e",
        "dev",
        "--var",
        &format!("base_url=http://{addr}"),
        "-p",
        "Hb",
    ];

    let o = run(&args, None, None, &[]).await;
    assert_eq!(o.status.code(), Some(3));
    assert!(
        err(&o).contains("secret variable `token` has no value"),
        "{}",
        err(&o)
    );

    let o = run(
        &args,
        None,
        None,
        &[("SNAPREQ_SECRET_TOKEN", "from-env-var")],
    )
    .await;
    let text = out(&o);
    assert!(
        text.contains("authorization: Bearer from-env-var\n"),
        "body shows what was sent: {text}"
    );
    assert!(
        text.contains("Authorization: Bearer <redacted>\n"),
        "request headers are redacted: {text}"
    );

    let o = run(
        &[
            "secret",
            "set",
            "token",
            "-e",
            "dev",
            "-c",
            dir.to_str().unwrap(),
        ],
        None,
        Some("stored-secret\n"),
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(err(&o).contains(".secrets"), "{}", err(&o));
    let o = run(&args, None, None, &[]).await;
    assert!(
        out(&o).contains("authorization: Bearer stored-secret\n"),
        "{}",
        out(&o)
    );
}

#[tokio::test]
async fn ls_shows_the_tree_and_environments() {
    let dir = collection("ls");
    let o = run(&["ls", dir.to_str().unwrap()], None, None, &[]).await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let text = out(&o);
    assert!(
        text.starts_with("Basic API\n├── GET     health\n├── users/  Users\n"),
        "{text}"
    );
    assert!(
        text.contains("│   ├── POST    create-user  Create user\n"),
        "{text}"
    );
    assert!(text.ends_with("environments: dev, prod\n"), "{text}");
    assert!(err(&o).contains("`missing-item`"), "warnings go to stderr");
}

#[tokio::test]
async fn new_collection_and_request() {
    let base = std::env::temp_dir().join(format!("snapreq-cli-new-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    let dir = base.join("shop-api");

    let o = run(
        &["new", "collection", dir.to_str().unwrap()],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(
        std::fs::read_to_string(dir.join("snapreq.toml")).unwrap(),
        "format = 1\nname = \"shop-api\"\n"
    );
    // A second time is refused.
    assert_eq!(
        run(
            &["new", "collection", dir.to_str().unwrap()],
            None,
            None,
            &[]
        )
        .await
        .status
        .code(),
        Some(3)
    );

    let req = dir.join("orders/create.toml");
    let o = run(
        &[
            "new",
            "request",
            req.to_str().unwrap(),
            "POST",
            "{{base_url}}/orders",
            "--name",
            "Create order",
        ],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert_eq!(
        std::fs::read_to_string(&req).unwrap(),
        "name = \"Create order\"\nmethod = \"POST\"\nurl = \"{{base_url}}/orders\"\n"
    );

    let outside = base.join("not-a-collection/x.toml");
    std::fs::create_dir_all(outside.parent().unwrap()).unwrap();
    let o = run(
        &["new", "request", outside.to_str().unwrap(), "http://a"],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(3));
    assert!(err(&o).contains("not inside a collection"), "{}", err(&o));
}

#[tokio::test]
async fn env_and_var_need_a_collection_request() {
    let o = run(
        &["send", "http://127.0.0.1:1/", "-e", "dev"],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(3));
    assert!(
        err(&o).contains("only apply to collection requests"),
        "{}",
        err(&o)
    );
}

#[tokio::test]
async fn collection_auth_is_applied_and_command_line_auth_wins() {
    let addr = server().await;
    let dir = collection("auth");
    let manifest = dir.join("snapreq.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        format!("{text}\n[auth]\ntype = \"api_key\"\nname = \"X-API-Key\"\nvalue = \"{{{{api_key}}}}\"\n"),
    )
    .unwrap();
    let base = format!("base_url=http://{addr}");
    let send = |extra: &'static [&'static str]| {
        let mut args = vec![
            "send",
            "-c",
            dir.to_str().unwrap(),
            "health",
            "--var",
            &base,
            "--var",
            "api_key=key-from-var",
        ];
        args.extend_from_slice(extra);
        let args: Vec<String> = args.into_iter().map(str::to_owned).collect();
        async move {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            run(&args, None, None, &[]).await
        }
    };

    let o = send(&[]).await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(out(&o).contains("x-api-key: key-from-var\n"), "{}", out(&o));

    let o = send(&["-u", "me:pw"]).await;
    let text = out(&o);
    assert!(text.contains("authorization: Basic bWU6cHc=\n"), "{text}");
    assert!(!text.contains("x-api-key"), "{text}");

    let o = send(&["--no-auth"]).await;
    let text = out(&o);
    assert!(
        !text.contains("x-api-key") && !text.contains("authorization"),
        "{text}"
    );
}

#[tokio::test]
async fn ls_marks_where_auth_is_set() {
    let dir = collection("ls-auth");
    let manifest = dir.join("snapreq.toml");
    let text = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(
        &manifest,
        format!("{text}\n[auth]\ntype = \"bearer\"\ntoken = \"{{{{token}}}}\"\n"),
    )
    .unwrap();
    let health = dir.join("health.toml");
    let text = std::fs::read_to_string(&health).unwrap();
    std::fs::write(&health, format!("{text}\n[auth]\ntype = \"none\"\n")).unwrap();

    let o = run(&["ls", dir.to_str().unwrap()], None, None, &[]).await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let text = out(&o);
    assert!(text.starts_with("Basic API  [auth: bearer]\n"), "{text}");
    assert!(text.contains("── GET     health  [auth: none]\n"), "{text}");
    assert!(
        text.contains("── GET     list-users  List users\n"),
        "{text}"
    );
}

#[tokio::test]
async fn request_file_assertions_run_after_the_response() {
    let addr = server().await;
    let dir = collection("asserts");
    let file = dir.join("check.toml");
    let write = |text: &str| {
        std::fs::write(&file, format!("url = \"{{{{base_url}}}}/check\"\n{text}")).unwrap()
    };
    let base = format!("base_url=http://{addr}");
    let send = |extra: &'static [&'static str]| {
        let mut args = vec![
            "send".to_owned(),
            file.to_str().unwrap().to_owned(),
            "--var".into(),
            base.clone(),
        ];
        args.extend(extra.iter().map(|s| (*s).to_owned()));
        async move {
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            run(&args, None, None, &[]).await
        }
    };

    write(
        "[assert]\nstatus = 200\nbody = { contains = \"GET /check\" }\n[assert.headers]\ncontent-length = { gt = 0 }\n",
    );
    let o = send(&[]).await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    assert!(err(&o).contains("✓ status equals 200\n"), "{}", err(&o));
    assert!(err(&o).ends_with("assertions: 3 passed\n"), "{}", err(&o));

    write("[assert]\nstatus = \"4xx\"\n[assert.json]\n\"$.id\" = 1\n");
    let o = send(&[]).await;
    assert_eq!(o.status.code(), Some(7), "{}", err(&o));
    assert!(
        err(&o).contains("✗ status equals \"4xx\"  (actual: 200)\n"),
        "{}",
        err(&o)
    );
    assert!(
        err(&o).contains("✗ json $.id equals 1  (the body is not JSON"),
        "{}",
        err(&o)
    );
    assert!(
        err(&o).ends_with("assertions: 0 passed, 2 failed\n"),
        "{}",
        err(&o)
    );

    assert_eq!(send(&["--no-assert"]).await.status.code(), Some(0));

    let o = send(&["--format", "json"]).await;
    assert_eq!(o.status.code(), Some(7));
    let doc: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(doc["assertions"][0]["subject"], "status");
    assert_eq!(doc["assertions"][0]["passed"], false);
    assert_eq!(doc["assertions"][0]["actual"], "200");

    write("[assert.json]\n\"$.[\" = 1\n");
    let o = send(&[]).await;
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    assert!(err(&o).contains("invalid JSONPath `$.[`"), "{}", err(&o));
}

#[tokio::test]
async fn request_file_captures_are_shown_with_v_and_in_json() {
    let addr = server().await;
    let dir = collection("captures");
    let file = dir.join("cap.toml");
    std::fs::write(
        &file,
        "url = \"{{base_url}}/cap\"\n[capture]\nline = { regex = '^(\\w+) (\\S+)', group = 2 }\nstatus = { status = true }\nsecret = { body = true, secret = true }\nmissing = { header = \"x-nope\" }\n",
    )
    .unwrap();
    let base = format!("base_url=http://{addr}");
    let o = run(
        &["send", file.to_str().unwrap(), "--var", &base, "-v"],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(0), "{}", err(&o));
    let e = err(&o);
    assert!(e.contains("captured line = /cap\n"), "{e}");
    assert!(e.contains("captured status = 200\n"), "{e}");
    assert!(e.contains("captured secret = <redacted>\n"), "{e}");
    assert!(
        e.contains("not captured missing: no `x-nope` header\n"),
        "{e}"
    );

    let o = run(
        &[
            "send",
            file.to_str().unwrap(),
            "--var",
            &base,
            "--format",
            "json",
        ],
        None,
        None,
        &[],
    )
    .await;
    let doc: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(
        doc["captures"][0],
        serde_json::json!({"name": "line", "value": "/cap", "secret": false})
    );
    assert_eq!(doc["captures"][2]["value"], "<redacted>");
    assert_eq!(doc["captures"][3]["error"], "no `x-nope` header");

    std::fs::write(
        &file,
        "url = \"{{base_url}}/cap\"\n[capture]\nx = { regex = \"(\" }\n",
    )
    .unwrap();
    let o = run(
        &["send", file.to_str().unwrap(), "--var", &base],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(3), "{}", err(&o));
    assert!(
        err(&o).contains("capture `x`: invalid regular expression"),
        "{}",
        err(&o)
    );
}
