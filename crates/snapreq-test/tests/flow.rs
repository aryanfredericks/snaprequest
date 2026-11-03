//! Flows against a stateful mock: log in → create → poll until ready → delete.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use snapreq_collection::{Collection, Secrets, Variables};
use snapreq_core::Client;
use snapreq_test::{RunOutcome, run_flow};
use tokio::net::TcpListener;

/// order id → polls so far
type Orders = Arc<Mutex<HashMap<u32, u32>>>;

fn reply(status: u16, body: &str) -> hyper::Response<Full<Bytes>> {
    hyper::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body.to_owned())))
        .unwrap()
}

/// - `POST /login` → `{"token": "tok-42xyz"}`
/// - `POST /orders` (bearer + `X-Team: shop` required) → 201 `{"id": n, "state": "pending"}`
/// - `GET /orders/n` → `pending` for the first two polls, then `ready`
/// - `DELETE /orders/n` → 204
async fn handle(
    orders: Orders,
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let header = |n: &str| {
        req.headers()
            .get(n)
            .map(|v| v.to_str().unwrap().to_owned())
            .unwrap_or_default()
    };
    let authorized = header("authorization") == "Bearer tok-42xyz";
    let team = header("x-team");
    let method = req.method().clone();
    let path = req.uri().path().to_owned();
    let body = req.into_body().collect().await.unwrap().to_bytes();
    if path == "/login" {
        return Ok(reply(200, r#"{"token": "tok-42xyz"}"#));
    }
    if !authorized {
        return Ok(reply(401, ""));
    }
    let mut orders = orders.lock().unwrap();
    Ok(match (method.as_str(), path.strip_prefix("/orders")) {
        ("POST", Some("")) if team == "shop" => {
            let item: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let id = orders.len() as u32 + 1;
            orders.insert(id, 0);
            reply(
                201,
                &format!(
                    r#"{{"id": {id}, "state": "pending", "item": {}}}"#,
                    item["item"]
                ),
            )
        }
        ("POST", Some("")) => reply(400, r#"{"error": "team"}"#),
        ("GET", Some(id)) => match orders.get_mut(&id.trim_start_matches('/').parse().unwrap_or(0))
        {
            Some(polls) => {
                *polls += 1;
                let state = if *polls > 2 { "ready" } else { "pending" };
                reply(200, &format!(r#"{{"state": "{state}", "polls": {polls}}}"#))
            }
            None => reply(404, ""),
        },
        ("DELETE", Some(_)) => reply(204, ""),
        _ => reply(404, ""),
    })
}

async fn start() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let orders = Orders::default();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            let orders = orders.clone();
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        TokioIo::new(s),
                        service_fn(move |r| handle(orders.clone(), r)),
                    )
                    .await;
            });
        }
    });
    addr
}

const CHECKOUT: &str = r#"
name = "Checkout"

[vars]
item = "sku-42"

[[step]]
request = "auth/login"

[[step]]
name = "Create order"
method = "POST"
url = "{{base}}/orders"
[step.body]
type = "json"
content = '{ "item": "{{item}}" }'
[step.assert]
status = 201
[step.assert.json]
"$.item" = "{{item}}"
[step.capture]
order_id = "$.id"

[[step]]
request = "orders/get"
[step.retry]
max = 10
every_ms = 10
[step.retry.until.json]
"$.state" = "ready"

[[step]]
name = "Delete"
method = "DELETE"
url = "{{base}}/orders/{{order_id}}"
[step.assert]
status = 204
"#;

fn collection(tag: &str, addr: SocketAddr, flows: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snapreq-flow-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut files = vec![
        (
            "snapreq.toml".to_owned(),
            format!("format = 1\nname = \"Shop\"\n[vars]\nbase = \"http://{addr}\"\n[auth]\ntype = \"bearer\"\ntoken = \"{{{{token}}}}\"\n"),
        ),
        (
            "auth/login.toml".into(),
            "method = \"POST\"\nurl = \"{{base}}/login\"\n[auth]\ntype = \"none\"\n[capture]\ntoken = { json = \"$.token\", secret = true }\n".into(),
        ),
        ("orders/folder.toml".into(), "[headers]\nX-Team = \"shop\"\n".into()),
        ("orders/get.toml".into(), "url = \"{{base}}/orders/{{order_id}}\"\n[assert]\nstatus = 200\n".into()),
    ];
    files.extend(
        flows
            .iter()
            .map(|(p, t)| ((*p).to_owned(), (*t).to_owned())),
    );
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

#[tokio::test]
async fn checkout_flow_passes_values_between_steps() {
    let addr = start().await;
    let dir = collection("ok", addr, &[("orders/checkout.flow.toml", CHECKOUT)]);
    let (c, _) = Collection::load(&dir).unwrap();
    // Flows are not requests.
    assert!(c.requests().iter().all(|(k, _)| !k.contains("checkout")));
    let flow = c.find_flow("orders/checkout").unwrap();
    let secrets = Secrets::default();
    let mut session = snapreq_test::Session::new(&c, None, &secrets, Client::default());

    let run = run_flow(&mut session, flow, |_| {}).await;
    let summary: Vec<(usize, &str, u32, bool)> = run
        .steps
        .iter()
        .map(|s| (s.number, s.title.as_str(), s.attempts, s.run.passed()))
        .collect();
    assert_eq!(
        summary,
        [
            (1, "auth/login", 1, true),
            (2, "Create order", 1, true),
            (3, "orders/get", 3, true),
            (4, "Delete", 1, true),
        ],
        "{run:#?}"
    );
    assert!(run.passed());
    // The token captured in step 1 is hidden in later traces.
    let trace = serde_json::to_string(&run.steps[3].run.response().unwrap().trace).unwrap();
    assert!(
        !trace.contains("tok-42xyz") && trace.contains("Bearer <redacted>"),
        "{trace}"
    );
    assert_eq!(
        run.steps[2].run.until[0].actual.as_deref(),
        Some("\"ready\"")
    );
}

#[tokio::test]
async fn variables_layer_and_failures_stop_the_flow() {
    let addr = start().await;
    let failing = CHECKOUT.replace("status = 201", "status = 202");
    let dir = collection(
        "fail",
        addr,
        &[
            ("checkout.flow.toml", &failing),
            ("poll.flow.toml", POLL_FOREVER),
        ],
    );
    let (c, _) = Collection::load(&dir).unwrap();
    let secrets = Secrets::default();

    // At the top level the inline step does not inherit `orders/folder.toml`, so X-Team is
    // missing and the server answers 400.
    let mut session = snapreq_test::Session::new(&c, None, &secrets, Client::default());
    let run = run_flow(&mut session, c.find_flow("checkout").unwrap(), |_| {}).await;
    assert!(!run.passed());
    assert_eq!(run.steps.len(), 2);
    assert_eq!(run.skipped(), 2);
    let failed: Vec<String> = run.steps[1]
        .run
        .assertions
        .iter()
        .filter(|a| !a.passed)
        .map(|a| format!("{} {} {:?}", a.subject, a.check, a.actual))
        .collect();
    assert_eq!(
        failed,
        [
            "status equals 202 Some(\"400\")",
            "json $.item equals \"sku-42\" Some(\"missing\")"
        ]
    );

    // The JUnit report shows the failure and the skipped steps.
    let flow = c.find_flow("checkout").unwrap();
    let its = [snapreq_test::Iteration {
        number: 1,
        data: None,
        result: snapreq_test::UnitRun::Flow(run),
    }];
    let report =
        snapreq_test::Report::new(&c, &snapreq_test::Unit::Flow(flow), &its, 1, Vec::new());
    let xml = report.junit();
    assert!(
        xml.contains("tests=\"4\" failures=\"1\" errors=\"0\" skipped=\"2\""),
        "{xml}"
    );
    assert!(
        xml.contains("name=\"3. orders/get\" time=\"0\">\n      <skipped/>"),
        "{xml}"
    );
    assert!(xml.contains("<failure type=\"assertion\" message=\"status equals 202 (actual: 400)\">status equals 202 (actual: 400)\njson $.item equals &quot;sku-42&quot; (missing)\ncapture order_id: `$.id` matched nothing</failure>"), "{xml}");
    assert!(!report.passed());
    assert!(
        report
            .html()
            .contains("<span class=\"muted\">skipped</span>")
    );

    // --var overrides win over flow variables.
    let mut session = snapreq_test::Session::new(&c, None, &secrets, Client::default());
    session.overrides.set("item", "from-cli");
    let scopes = session.scopes_with(
        "checkout",
        Some(&c.find_flow("checkout").unwrap().file.vars),
        None,
    );
    assert_eq!(
        scopes.lookup("item").unwrap().0.value.as_deref(),
        Some("from-cli")
    );
    let mut step_vars = Variables::default();
    step_vars.set("item", "from-step");
    let mut session = snapreq_test::Session::new(&c, None, &secrets, Client::default());
    session.runtime.set("item", "captured");
    let scopes = session.scopes_with(
        "checkout",
        Some(&c.find_flow("checkout").unwrap().file.vars),
        Some(&step_vars),
    );
    assert_eq!(
        scopes.lookup("item").unwrap().0.value.as_deref(),
        Some("from-step")
    );

    // Retries end after `max` attempts with the failing `until` check.
    let mut session = snapreq_test::Session::new(&c, None, &secrets, Client::default());
    let run = run_flow(&mut session, c.find_flow("poll").unwrap(), |_| {}).await;
    let last = run.steps.last().unwrap();
    assert_eq!(last.attempts, 3);
    assert!(!last.run.passed());
    assert_eq!(last.run.until[0].check, "equals \"never\"");

    // A step that cannot be built is not retried.
    let broken = POLL_FOREVER.replace("{{base}}/login", "{{undefined}}/login");
    let dir = collection("broken", addr, &[("poll.flow.toml", &broken)]);
    let (c, _) = Collection::load(&dir).unwrap();
    let mut session = snapreq_test::Session::new(&c, None, &secrets, Client::default());
    let run = run_flow(&mut session, c.find_flow("poll").unwrap(), |_| {}).await;
    assert_eq!(run.steps[0].attempts, 1);
    assert!(
        matches!(&run.steps[0].run.outcome, RunOutcome::Invalid(why) if why.contains("`undefined`"))
    );
}

const POLL_FOREVER: &str = r#"
[[step]]
method = "POST"
url = "{{base}}/login"
[step.auth]
type = "none"
[step.retry]
max = 3
every_ms = 1
[step.retry.until.json]
"$.token" = "never"
"#;
