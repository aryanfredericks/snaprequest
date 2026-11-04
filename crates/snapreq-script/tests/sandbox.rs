//! Scripts (PLAN P9.3): the `snapreq` API, and the sandbox they cannot leave.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::time::{Duration, Instant};

use serde_json::json;
use snapreq_script::{
    Error, Header, Input, Limits, Output, Phase, ScriptRequest, ScriptResponse, run,
};

fn input(phase: Phase) -> Input {
    let vars = json!({ "base": "https://api.test", "token": "t0k" });
    Input {
        phase,
        vars: vars.as_object().unwrap().clone(),
        request: ScriptRequest {
            method: "GET".into(),
            url: "{{base}}/users".into(),
            headers: vec![Header {
                name: "Accept".into(),
                value: "application/json".into(),
            }],
            body: None,
        },
        response: (phase == Phase::Test).then(|| ScriptResponse {
            status: 201,
            headers: vec![Header {
                name: "Content-Type".into(),
                value: "application/json".into(),
            }],
            body:
                r#"{"id": 7, "name": "snap", "tags": ["a", "b"], "owner": {"email": "a@b.test"}}"#
                    .into(),
            time_ms: 12.5,
        }),
    }
}

fn ok(code: &str, phase: Phase) -> Output {
    run(code, &input(phase), &Limits::default()).unwrap()
}

#[test]
fn pre_request_scripts_set_variables_and_change_the_request() {
    let out = ok(
        r#"
        const t = snapreq.variables.get("token");
        snapreq.variables.set("ts", 1700000000);
        snapreq.variables.setSecret("sig", "sig-" + t);
        snapreq.request.headers.set("X-Signature", "{{sig}}");
        snapreq.request.headers.remove("accept");
        snapreq.request.method = "post";
        snapreq.request.url = snapreq.request.url + "?ts={{ts}}";
        snapreq.request.body = { hello: "world" };
        console.log("signed", { ok: true }, 3);
        console.warn("careful");
        "#,
        Phase::PreRequest,
    );
    assert_eq!(out.error, None);
    let vars: Vec<_> = out
        .vars
        .iter()
        .map(|v| (v.name.as_str(), v.value.as_str(), v.secret))
        .collect();
    assert_eq!(
        vars,
        [("ts", "1700000000", false), ("sig", "sig-t0k", true)]
    );
    assert_eq!(out.request.method, "POST");
    assert_eq!(out.request.url, "{{base}}/users?ts={{ts}}");
    assert_eq!(out.request.headers.len(), 1);
    assert_eq!(out.request.headers[0].name, "X-Signature");
    assert_eq!(out.request.body.as_deref(), Some(r#"{"hello":"world"}"#));
    assert_eq!(out.logs, ["signed {\"ok\":true} 3", "warn: careful"]);
}

#[test]
fn test_scripts_check_the_response_in_jest_and_chai_style() {
    let out = ok(
        r#"
        const r = snapreq.response;
        snapreq.test("status", () => snapreq.expect(r.status).toBe(201));
        snapreq.test("chai", () => {
            snapreq.expect(r.status).to.equal(201);
            snapreq.expect(r.json()).to.have.property("name", "snap");
            snapreq.expect(r.json().tags).to.include("b").and.to.have.lengthOf(2);
            snapreq.expect(r.json().id).to.be.a("number").and.to.be.above(5);
            snapreq.expect(r.json()).to.deep.include({ owner: { email: "a@b.test" } });
            snapreq.expect(r.headers.get("content-type")).to.match(/json/);
            snapreq.expect(r.time).to.be.below(1000);
            snapreq.expect(true).to.be.true;
            snapreq.expect(null).to.not.exist;
        });
        snapreq.test("jest", () => {
            snapreq.expect(r.json()).toEqual(expect_doc());
            snapreq.expect(r.json()).toHaveProperty("owner.email", "a@b.test");
            snapreq.expect([1, 2]).not.toContain(3);
            snapreq.expect(r.json().id).toBeTypeOf("integer");
        });
        function expect_doc() {
            return { id: 7, name: "snap", tags: ["a", "b"], owner: { email: "a@b.test" } };
        }
        snapreq.test("fails", () => snapreq.expect(r.json().name).toBe("other"));
        snapreq.test("throws", () => { throw new Error("boom"); });
        snapreq.test("async", async () => {});
        snapreq.variables.set("user_id", r.json().id);
        "#,
        Phase::Test,
    );
    assert_eq!(out.error, None);
    let results: Vec<_> = out
        .tests
        .iter()
        .map(|t| {
            (
                t.name.as_str(),
                t.passed,
                t.error.clone().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        results,
        [
            ("status", true, String::new()),
            ("chai", true, String::new()),
            ("jest", true, String::new()),
            (
                "fails",
                false,
                "expected \"snap\" to be \"other\"".to_owned()
            ),
            ("throws", false, "boom".to_owned()),
            (
                "async",
                false,
                "asynchronous tests are not supported".to_owned()
            ),
        ]
    );
    assert_eq!(out.vars[0].value, "7");
}

#[test]
fn errors_are_reported_and_what_ran_before_counts() {
    let out = ok(
        "snapreq.variables.set('a', 1); null.boom;",
        Phase::PreRequest,
    );
    assert_eq!(out.vars.len(), 1);
    assert!(
        out.error.as_deref().unwrap().starts_with("TypeError"),
        "{out:?}"
    );
    let out = ok("this is not javascript", Phase::PreRequest);
    assert!(
        out.error.as_deref().unwrap().starts_with("SyntaxError"),
        "{out:?}"
    );
    let out = ok("throw 'plain'", Phase::PreRequest);
    assert_eq!(out.error.as_deref(), Some("uncaught plain"));
    // The response cannot be changed, nor the request after it was sent.
    let out = ok("snapreq.request.url = 'x'", Phase::Test);
    assert!(
        out.error
            .unwrap()
            .contains("only be changed in a pre-request")
    );
    let out = ok("snapreq.response.headers.set('a', 'b')", Phase::Test);
    assert!(out.error.unwrap().contains("cannot be changed"));
    let out = ok("snapreq.variables.set('bad name', 1)", Phase::Test);
    assert!(out.error.unwrap().contains("invalid variable name"));
    // Replacing globals does not break the result.
    let out = ok(
        "JSON.stringify = () => 'x'; snapreq.variables.set('k', 'v')",
        Phase::Test,
    );
    assert_eq!(out.vars[0].value, "v");
}

#[test]
fn scripts_have_no_way_out_of_the_sandbox() {
    let out = ok(
        r#"
        const g = Function("return this")();
        const found = ["require", "process", "fetch", "XMLHttpRequest", "WebSocket", "std", "os",
                       "setTimeout", "setInterval", "Deno", "Bun", "module", "exports", "__dirname",
                       "importScripts", "navigator", "scriptArgs"]
            .filter((n) => n in g || typeof g[n] !== "undefined");
        console.log(JSON.stringify(found));
        "#,
        Phase::PreRequest,
    );
    assert_eq!(out.logs, ["[]"], "{out:?}");

    // No module loader: dynamic import cannot load anything (it rejects, never resolves).
    let out = ok(
        r#"
        let state = "pending";
        import("std").then(() => { state = "loaded"; }, (e) => { state = "rejected"; });
        console.log(state);
        "#,
        Phase::PreRequest,
    );
    assert_ne!(out.logs.first().map(String::as_str), Some("loaded"));
    let out = ok("import fs from 'fs';", Phase::PreRequest);
    assert!(out.error.unwrap().starts_with("SyntaxError"));

    // Only the input document is visible: no other request's data, no host objects.
    let out = ok(
        "console.log(Object.keys(snapreq.variables.toObject()).join(','))",
        Phase::PreRequest,
    );
    assert_eq!(out.logs, ["base,token"]);
}

#[test]
fn endless_loops_are_stopped() {
    let limits = Limits {
        time: Duration::from_millis(200),
        ..Limits::default()
    };
    for code in [
        "while (true) {}",
        // Catching does not help: the interrupt cannot be caught.
        "for (;;) { try { while (true) {} } catch (e) {} }",
        "snapreq.test('t', () => { while (true) {} })",
    ] {
        let start = Instant::now();
        let err = run(code, &input(Phase::Test), &limits).unwrap_err();
        assert!(matches!(err, Error::Timeout(200)), "{code}: {err}");
        assert!(start.elapsed() < Duration::from_secs(2), "{code}");
    }
}

#[test]
fn memory_and_stack_are_limited() {
    let limits = Limits {
        memory: 8 << 20,
        ..Limits::default()
    };
    let bomb = "let a = []; for (;;) a.push('x'.repeat(1 << 16) + a.length);";
    match run(bomb, &input(Phase::Test), &limits) {
        Ok(out) => assert!(
            out.error
                .as_deref()
                .is_some_and(|e| e.contains("out of memory")),
            "{out:?}"
        ),
        Err(Error::Engine(e)) => assert!(e.contains("memory") || e.contains("Exception"), "{e}"),
        Err(e) => panic!("{e}"),
    }
    let out = ok("function f(n) { return f(n + 1) + 1; } f(0);", Phase::Test);
    assert!(
        out.error
            .as_deref()
            .is_some_and(|e| e.contains("stack size exceeded") || e.contains("stack overflow")),
        "{out:?}"
    );
    // A fresh engine each time: nothing survives between runs.
    ok("globalThis.leak = 'x'", Phase::Test);
    let out = ok("console.log(typeof globalThis.leak)", Phase::Test);
    assert_eq!(out.logs, ["undefined"]);
}
