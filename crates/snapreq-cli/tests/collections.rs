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
        "url = \"{{base_url}}/cap\"\n[capture]\nline = { regex = '^(\\w+) (\\S+)', group = 2 }\nstatus = { status = true }\nsecret = { body = true, secret = true }\nmissing = { header = \"x-nope\" }\n[assert.body]\nequals = \"wrong\"\n",
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
    assert_eq!(o.status.code(), Some(7), "{}", err(&o));
    let e = err(&o);
    // The failing check would print the body, which was captured as secret.
    assert!(
        e.contains("✗ body equals \"wrong\"  (actual: \"<redacted>\")"),
        "{e}"
    );
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

#[tokio::test]
async fn run_executes_a_flow_and_reports_each_step() {
    let addr = server().await;
    let dir = collection("flow");
    std::fs::write(
        dir.join("users/onboard.flow.toml"),
        r#"name = "Onboard"
[vars]
who = "flow-default"

[[step]]
name = "First"
url = "{{base_url}}/first/{{who}}"
[step.capture]
path = { regex = '^GET (\S+)' }
[step.assert]
status = 200

[[step]]
request = "users/list-users"
vars = { section = "from-step" }
[step.assert.body]
contains = "x-section: users"

[[step]]
name = "Uses capture"
url = "{{base_url}}/echo"
[step.headers]
X-Prev = "{{path}}"
[step.assert.body]
contains = "{{expect}}"
"#,
    )
    .unwrap();
    let base = format!("base_url=http://{addr}");
    let flow = dir.join("users/onboard.flow.toml");
    let flow = flow.to_str().unwrap();

    let o = run(&["ls", dir.to_str().unwrap()], None, None, &[]).await;
    assert!(out(&o).contains("flows: users/onboard\n"), "{}", out(&o));

    let o = run(
        &[
            "run",
            flow,
            "--var",
            &base,
            "--var",
            "who=cli",
            "--var",
            "expect=x-prev: /first/cli",
            "--color",
            "never",
        ],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(0), "{}{}", out(&o), err(&o));
    let text = out(&o);
    assert!(text.starts_with("Onboard  users/onboard\n"), "{text}");
    assert!(
        text.contains("✓  1  First             GET http://"),
        "{text}"
    );
    assert!(
        text.contains("✓  2  users/list-users  GET http://"),
        "{text}"
    );
    assert!(text.ends_with("flow passed: 3 of 3 steps\n"), "{text}");

    // A failing check stops the flow: exit 7, the failure is shown, the rest is skipped.
    let o = run(
        &[
            "run",
            "-c",
            dir.to_str().unwrap(),
            "users/onboard",
            "--var",
            &base,
            "--var",
            "expect=nope",
            "--color",
            "never",
        ],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(7), "{}", out(&o));
    let text = out(&o);
    assert!(text.contains("✗  3  Uses capture"), "{text}");
    assert!(
        text.contains("      ✗ body contains \"nope\"  (actual: "),
        "{text}"
    );
    assert!(text.contains("flow failed: 2 of 3 steps passed"), "{text}");

    let o = run(
        &[
            "run",
            flow,
            "--var",
            &base,
            "--var",
            "expect=nope",
            "--format",
            "json",
        ],
        None,
        None,
        &[],
    )
    .await;
    let doc: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(doc["passed"], false);
    assert_eq!(
        doc["iterations"][0]["steps"][0]["captures"][0]["value"],
        "/first/flow-default"
    );
    assert_eq!(
        doc["iterations"][0]["steps"][2]["assertions"][0]["passed"],
        false
    );
    assert_eq!(doc["iterations"][0]["steps"][1]["response"]["status"], 200);

    // Undefined variables: the step is not sent, exit 3.
    let o = run(
        &["run", flow, "--var", &base, "--color", "never"],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(3), "{}", out(&o));
    assert!(
        out(&o).contains("variable `expect` is not defined"),
        "{}",
        out(&o)
    );

    let o = run(
        &["run", "-c", dir.to_str().unwrap(), "nope"],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(3));
    assert!(
        err(&o).contains("no flow or folder `nope` in the collection (flows: users/onboard)"),
        "{}",
        err(&o)
    );
}

#[tokio::test]
async fn run_a_collection_a_folder_and_data_rows() {
    let addr = server().await;
    let dir = collection("run-all");
    std::fs::write(
        dir.join("users/hello.toml"),
        "url = \"{{base_url}}/hello/{{who}}\"\n[vars]\nwho = \"nobody\"\n[assert.body]\nnot_contains = \"/hello/bob\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("people.csv"), "who\nalice\nbob\n").unwrap();
    let base = format!("base_url=http://{addr}");
    let d = dir.to_str().unwrap();
    let common = [
        "--var",
        base.as_str(),
        "--var",
        "token=t",
        "--color",
        "never",
    ];

    let o = run(&[&["run", d][..], &common].concat(), None, None, &[]).await;
    let text = out(&o);
    assert!(text.starts_with("Basic API  collection\n"), "{text}");
    for key in [
        "health",
        "users/list-users",
        "users/create-user",
        "users/hello",
        "zeta",
    ] {
        assert!(text.contains(&format!("  {key}")), "{key}: {text}");
    }

    let users = dir.join("users");
    let o = run(
        &[&["run", users.to_str().unwrap()][..], &common].concat(),
        None,
        None,
        &[],
    )
    .await;
    let text = out(&o);
    assert!(text.starts_with("Basic API  users\n"), "{text}");
    assert!(!text.contains("health"), "{text}");

    // One iteration per CSV row; the row beats the request's own variable.
    let csv = dir.join("people.csv");
    let o = run(
        &[
            &[
                "run",
                "-c",
                d,
                "--folder",
                "users",
                "--data",
                csv.to_str().unwrap(),
            ][..],
            &common,
        ]
        .concat(),
        None,
        None,
        &[],
    )
    .await;
    let text = out(&o);
    assert!(text.contains("iteration 1/2  who=alice\n"), "{text}");
    assert!(text.contains("iteration 2/2  who=bob\n"), "{text}");
    assert!(
        text.contains("✗ body not contains \"/hello/bob\""),
        "{text}"
    );
    assert!(text.ends_with("of 2 iterations failed\n"), "{text}");
    assert_ne!(o.status.code(), Some(0));

    let o = run(
        &[
            &["run", "-c", d, "users", "-n", "3", "--format", "json"][..],
            &common[..4],
        ]
        .concat(),
        None,
        None,
        &[],
    )
    .await;
    let doc: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    assert_eq!(
        doc["target"],
        serde_json::json!({"kind": "folder", "key": "users", "name": "Basic API"})
    );
    assert_eq!(doc["iterations"].as_array().unwrap().len(), 3);

    let o = run(
        &["run", "-c", d, "--data", "x.csv", "-n", "2"],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(
        o.status.code(),
        Some(2),
        "conflicting options are a usage error"
    );
    let o = run(&["run", "-c", d, "--folder", "nope"], None, None, &[]).await;
    assert_eq!(o.status.code(), Some(3));
    assert!(
        err(&o).contains("no requests in folder `nope`"),
        "{}",
        err(&o)
    );
}

#[tokio::test]
async fn run_bail_parallel_and_delay_options() {
    let addr = server().await;
    let dir = collection("bail");
    std::fs::write(
        dir.join("users/hello.toml"),
        "url = \"{{base_url}}/hello/{{who}}\"\n[assert.body]\nnot_contains = \"/hello/bob\"\n",
    )
    .unwrap();
    let csv = dir.join("people.csv");
    std::fs::write(&csv, "who\nbob\nalice\ncarol\n").unwrap();
    let base = format!("base_url=http://{addr}");
    let d = dir.to_str().unwrap();
    let args = [
        "run",
        "-c",
        d,
        "users",
        "--data",
        csv.to_str().unwrap(),
        "--var",
        base.as_str(),
        "--var",
        "token=t",
        "--color",
        "never",
    ];

    let o = run(&[&args[..], &["--bail"]].concat(), None, None, &[]).await;
    assert_eq!(o.status.code(), Some(7), "{}", out(&o));
    let text = out(&o);
    assert!(text.contains("iteration 1/3  who=bob\n"), "{text}");
    assert!(!text.contains("iteration 2/3"), "{text}");
    assert!(text.contains("skipped"), "{text}");
    assert!(
        text.ends_with("1 of 3 iterations failed, 2 skipped\n"),
        "{text}"
    );

    let o = run(
        &[
            &args[..],
            &["--parallel", "3", "--delay", "1", "--format", "json"],
        ]
        .concat(),
        None,
        None,
        &[],
    )
    .await;
    let doc: serde_json::Value = serde_json::from_str(&out(&o)).unwrap();
    let numbers: Vec<u64> = doc["iterations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["number"].as_u64().unwrap())
        .collect();
    assert_eq!(numbers, [1, 2, 3]);
    assert_eq!(doc["planned_iterations"], 3);

    let o = run(&[&args[..], &["--parallel", "0"]].concat(), None, None, &[]).await;
    assert_eq!(o.status.code(), Some(3));
    assert!(
        err(&o).contains("--parallel must be at least 1"),
        "{}",
        err(&o)
    );
}

#[tokio::test]
async fn run_writes_junit_html_and_json_reports_without_secrets() {
    let addr = server().await;
    let dir = collection("reports");
    std::fs::write(
        dir.join("users/hello.toml"),
        "url = \"{{base_url}}/hello/{{who}}\"\n[assert.body]\nnot_contains = \"/hello/bob\"\n",
    )
    .unwrap();
    let csv = dir.join("people.csv");
    std::fs::write(&csv, "who\nalice\nbob\n").unwrap();
    let out_dir = dir.join("reports");
    let junit = out_dir.join("junit.xml");
    let html = out_dir.join("report.html");
    let json = out_dir.join("run.json");
    let base = format!("base_url=http://{addr}");
    let o = run(
        &[
            "run",
            "-c",
            dir.to_str().unwrap(),
            "users",
            "-e",
            "dev",
            "--data",
            csv.to_str().unwrap(),
            "--var",
            &base,
            "--var",
            "token=sup3r-secret-tok",
            "--report",
            &format!("junit:{}", junit.display()),
            "--report",
            &format!("html:{}", html.display()),
            "--report",
            &format!("json:{}", json.display()),
        ],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(7), "{}{}", out(&o), err(&o));
    assert!(
        err(&o).contains(&format!("report written to {}", junit.display())),
        "{}",
        err(&o)
    );

    let xml = std::fs::read_to_string(&junit).unwrap();
    assert!(xml.starts_with("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"Basic API\" tests=\"8\" failures=\"1\" errors=\"0\" skipped=\"0\""), "{xml}");
    assert_eq!(xml.matches("<testsuite ").count(), 2, "{xml}");
    assert!(
        xml.contains("name=\"Basic API (users) [iteration 2: who=bob]\""),
        "{xml}"
    );
    assert!(
        xml.contains("<testcase classname=\"users\" name=\"3. users/hello\""),
        "{xml}"
    );
    assert!(xml.contains("<failure type=\"assertion\" message=\"body not contains &quot;/hello/bob&quot; (actual: "), "{xml}");

    let page = std::fs::read_to_string(&html).unwrap();
    assert!(page.starts_with("<!DOCTYPE html>"), "{page}");
    assert!(
        page.contains("users/create-user") && page.contains("Iteration 2"),
        "self-contained page"
    );
    assert!(
        !page.contains("<script") && !page.contains("http-equiv") && !page.contains("<link"),
        "no external assets"
    );
    // The echo server sends the Authorization header back in the body.
    assert!(
        page.contains("authorization: Bearer &lt;redacted&gt;"),
        "body secrets hidden"
    );

    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&json).unwrap()).unwrap();
    let step = &doc["iterations"][0]["steps"][1];
    assert_eq!(step["key"], "users/create-user");
    assert!(step["response"]["trace"].as_array().unwrap().len() > 3);
    assert!(step["response"]["headers"].is_array());

    for text in [xml, page, doc.to_string()] {
        assert!(!text.contains("sup3r-secret-tok"), "secret leaked: {text}");
    }

    let o = run(
        &[
            "run",
            "-c",
            dir.to_str().unwrap(),
            "users",
            "--report",
            "pdf:x.pdf",
        ],
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(o.status.code(), Some(3));
    assert!(
        err(&o).contains("unknown report format `pdf`"),
        "{}",
        err(&o)
    );
}

#[tokio::test]
async fn import_a_public_postman_collection_and_run_it() {
    let addr = server().await;
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("../snapreq-interop/tests/fixtures");
    // Newman's sample collection, pointed at the local echo server instead of postman-echo.com.
    let sample = std::fs::read_to_string(fixtures.join("postman-newman-sample.json"))
        .unwrap()
        .replace("https://postman-echo.com", &format!("http://{addr}"));
    let work = std::env::temp_dir().join(format!("snapreq-cli-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(work.join("sample.json"), sample).unwrap();

    let o = run(&["import", "sample.json", "--json"], Some(&work), None, &[]).await;
    assert!(o.status.success(), "{}", err(&o));
    let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(doc["format"], "postman");
    assert_eq!(doc["requests"], 3);
    assert_eq!(doc["dir"], "sample-postman-collection");
    assert!(
        err(&o).contains("warning: 1 Postman script(s)"),
        "{}",
        err(&o)
    );

    let dir = work.join("sample-postman-collection");
    let o = run(
        &["run", dir.to_str().unwrap(), "--color", "never"],
        None,
        None,
        &[],
    )
    .await;
    assert!(o.status.success(), "{}{}", out(&o), err(&o));
    let o = run(
        &[
            "send",
            "-c",
            dir.to_str().unwrap(),
            "a-simple-post-request-with-json-body",
        ],
        None,
        None,
        &[],
    )
    .await;
    let echoed = out(&o);
    assert!(echoed.contains("POST /post"), "{echoed}");
    assert!(
        echoed.contains("content-type: application/json"),
        "{echoed}"
    );
    assert!(echoed.contains("Duis posuere"), "{echoed}");

    // Importing again into the same directory is refused.
    let o = run(&["import", "sample.json"], Some(&work), None, &[]).await;
    assert!(!o.status.success());
    assert!(err(&o).contains("not empty"), "{}", err(&o));
}

#[tokio::test]
async fn import_a_curl_command_from_stdin_keeps_the_token_secret() {
    let addr = server().await;
    let work = std::env::temp_dir().join(format!("snapreq-cli-import-curl-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).unwrap();
    let cmd = format!("curl http://{addr}/me -H 'Authorization: Bearer tok-from-clipboard'\n");
    let o = run(
        &["import", "-", "--into", "scratch", "--name", "Scratch"],
        Some(&work),
        Some(&cmd),
        &[],
    )
    .await;
    assert!(o.status.success(), "{}", err(&o));
    assert!(err(&o).contains("1 secret value stored"), "{}", err(&o));
    let file = std::fs::read_to_string(work.join("scratch/get-me.toml")).unwrap();
    assert!(
        !file.contains("tok-from-clipboard") && file.contains("{{token}}"),
        "{file}"
    );
    let o = run(&["send", "-c", "scratch", "get-me"], Some(&work), None, &[]).await;
    assert!(o.status.success(), "{}", err(&o));
    // The echo shows the header was sent (secret values are masked in output).
    assert!(out(&o).contains("authorization: Bearer"), "{}", out(&o));
    let o = run(
        &["import", "-", "--format", "yaml"],
        Some(&work),
        Some(""),
        &[],
    )
    .await;
    assert!(err(&o).contains("unknown format `yaml`"), "{}", err(&o));
}

#[tokio::test]
async fn send_export_prints_code_with_secrets_from_the_environment() {
    let dir = collection("export");
    let d = dir.to_str().unwrap();
    let o = run(
        &[
            "send",
            "-c",
            d,
            "users/create-user",
            "-e",
            "dev",
            "--export",
            "curl",
        ],
        None,
        None,
        &[("SNAPREQ_SECRET_TOKEN", "tok-from-env")],
    )
    .await;
    assert!(o.status.success(), "{}", err(&o));
    let text = out(&o);
    assert!(
        text.starts_with("curl \\\n  'http://localhost:8080/users'"),
        "{text}"
    );
    assert!(
        text.contains("-H \"Authorization: Bearer ${SNAPREQ_SECRET_TOKEN}\""),
        "{text}"
    );
    assert!(
        text.contains("--data-raw '{\n  \"name\": \"snap\""),
        "{text}"
    );
    assert!(!text.contains("tok-from-env"));
    assert!(!text.contains(" -L"), "follow_redirects = false: {text}");

    // A --var value for a declared secret is not written either.
    let o = run(
        &[
            "send",
            "-c",
            d,
            "users/create-user",
            "-e",
            "dev",
            "--var",
            "token=cli-secret",
            "--export",
            "python",
        ],
        None,
        None,
        &[],
    )
    .await;
    let text = out(&o);
    assert!(
        text.contains("os.environ[\"SNAPREQ_SECRET_TOKEN\"]"),
        "{text}"
    );
    assert!(!text.contains("cli-secret"), "{text}");

    // A plain URL; nothing is sent.
    let o = run(
        &[
            "send",
            "PUT",
            "https://a.test/x",
            "-H",
            "X-A: 1",
            "--export",
            "javascript",
        ],
        None,
        None,
        &[],
    )
    .await;
    assert!(
        out(&o).contains("await fetch(\"https://a.test/x\", {\n  method: \"PUT\""),
        "{}",
        out(&o)
    );
    let o = run(
        &["send", "https://a.test", "--export", "cobol"],
        None,
        None,
        &[],
    )
    .await;
    assert!(!o.status.success());
    assert!(err(&o).contains("unknown export format"), "{}", err(&o));
}

#[tokio::test]
async fn run_writes_a_har_report() {
    let addr = server().await;
    let dir = collection("har");
    let har = dir.join("out/run.har");
    let o = run(
        &[
            "run",
            "-c",
            dir.to_str().unwrap(),
            "users",
            "-e",
            "dev",
            "--var",
            &format!("base_url=http://{addr}"),
            "--var",
            "token=har-secret-tok",
            "--report",
            &format!("har:{}", har.display()),
        ],
        None,
        None,
        &[],
    )
    .await;
    assert!(err(&o).contains("report written to"), "{}", err(&o));
    let text = std::fs::read_to_string(&har).unwrap();
    assert!(!text.contains("har-secret-tok"), "{text}");
    let doc: serde_json::Value = serde_json::from_str(&text).unwrap();
    let entries = doc["log"]["entries"].as_array().unwrap();
    assert!(entries.len() >= 2, "{text}");
    let create = entries
        .iter()
        .find(|e| e["request"]["method"] == "POST")
        .unwrap();
    assert_eq!(create["response"]["status"], 200);
    assert!(
        create["response"]["content"]["text"]
            .as_str()
            .unwrap()
            .starts_with("POST /users")
    );
    assert_eq!(
        create["request"]["postData"]["mimeType"],
        "application/json"
    );
    assert_eq!(doc["log"]["creator"]["name"], "SnapRequest");
}

#[tokio::test]
async fn send_and_run_execute_scripts() {
    let addr = server().await;
    let dir = collection("scripts");
    std::fs::write(
        dir.join("users/scripted.toml"),
        r#"url = "{{base_url}}/scripted"

[script]
pre_request = '''
snapreq.variables.set("n", 21 * 2);
snapreq.request.headers.set("X-Answer", "{{n}}");
console.log("token is", snapreq.variables.get("token"));
'''
test = '''
snapreq.test("echoes the answer", () => snapreq.expect(snapreq.response.text()).to.include("x-answer: 42"));
snapreq.test("is a teapot", () => snapreq.expect(snapreq.response.status).toBe(418));
'''
"#,
    )
    .unwrap();
    let base = format!("base_url=http://{addr}");
    let d = dir.to_str().unwrap();
    let o = run(
        &[
            "send",
            "-c",
            d,
            "users/scripted",
            "-e",
            "dev",
            "--var",
            &base,
            "--var",
            "token=scr1pt-secret",
        ],
        None,
        None,
        &[],
    )
    .await;
    let (stdout, stderr) = (out(&o), err(&o));
    assert!(stdout.contains("x-answer: 42"), "{stdout}");
    assert!(stderr.contains("log: token is <redacted>"), "{stderr}");
    assert!(!stderr.contains("scr1pt-secret"), "{stderr}");
    assert!(stderr.contains("test is a teapot"), "{stderr}");
    assert_eq!(
        o.status.code(),
        Some(7),
        "a failed test fails the command: {stderr}"
    );

    let o = run(
        &[
            "run",
            "-c",
            d,
            "users",
            "-e",
            "dev",
            "--var",
            &base,
            "--var",
            "token=scr1pt-secret",
            "--color",
            "never",
        ],
        None,
        None,
        &[],
    )
    .await;
    let text = out(&o);
    assert!(text.contains("✗ test is a teapot"), "{text}");
    assert!(text.contains("log: token is <redacted>"), "{text}");
    assert!(!text.contains("scr1pt-secret"), "{text}");
}
