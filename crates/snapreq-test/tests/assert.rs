//! Evaluating assertions against responses.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::time::Duration;

use snapreq_collection::AssertSpec;
use snapreq_core::Response;
use snapreq_core::model::{Header, HttpVersion, Sizes, Timings};
use snapreq_test::{Assertions, Outcome};

fn response(status: u16, headers: &[(&str, &str)], body: &str, ms: u64) -> Response {
    Response {
        status,
        version: HttpVersion::Http11,
        url: "http://a.test/".into(),
        headers: headers.iter().map(|(n, v)| Header::new(*n, *v)).collect(),
        body: bytes::Bytes::from(body.to_owned()),
        remote_addr: None,
        timings: Timings {
            total: Duration::from_millis(ms),
            ..Timings::default()
        },
        sizes: Sizes::default(),
        trace: vec![],
    }
}

fn spec(text: &str) -> AssertSpec {
    #[derive(serde::Deserialize)]
    struct F {
        assert: AssertSpec,
    }
    toml::from_str::<F>(text).unwrap().assert
}

fn run(text: &str, resp: &Response) -> Vec<Outcome> {
    Assertions::compile(&spec(text)).unwrap().evaluate(resp)
}

/// `subject check` of every failed outcome.
fn failed(outcomes: &[Outcome]) -> Vec<String> {
    outcomes
        .iter()
        .filter(|o| !o.passed)
        .map(|o| format!("{} {}", o.subject, o.check))
        .collect()
}

const USER: &str = r#"{"id": 7, "name": "snap", "score": 9.5, "tags": ["a", "b"], "admin": false, "meta": null, "items": [{"n": 1}, {"n": 2}]}"#;

#[test]
fn passing_checks() {
    let resp = response(
        201,
        &[
            ("content-type", "application/json; charset=utf-8"),
            ("x-count", "3"),
            ("set-cookie", "a=1"),
            ("set-cookie", "b=2"),
        ],
        USER,
        120,
    );
    let outcomes = run(
        r#"
        [assert]
        status = "2xx"
        duration_ms = { lt = 500, gte = 100 }
        body = { contains = "snap", not_contains = "error", matches = '"id":\s*7' }

        [assert.headers]
        Content-Type = { starts_with = "application/json" }
        x-count = 3
        set-cookie = { contains = "b=2" }
        x-missing = { exists = false }

        [assert.json]
        "$.id" = { type = "integer", equals = 7.0, one_of = [6, 7] }
        "$.name" = "snap"
        "$.score" = { type = "number", gt = 9 }
        "$.tags" = { length = 2, contains = "a", equals = ["a", "b"] }
        "$" = { contains = "admin", type = "object" }
        "$.admin" = false
        "$.meta" = { type = "null", exists = true }
        "$.items[*].n" = [1, 2]
        "$.nope" = { exists = false }
        "#,
        &resp,
    );
    assert!(failed(&outcomes).is_empty(), "{outcomes:#?}");
    assert_eq!(outcomes.len(), 26);
    assert!(outcomes.iter().any(
        |o| o.subject == "header Content-Type" && o.check == "starts with \"application/json\""
    ));
}

#[test]
fn failures_show_expected_and_actual() {
    let resp = response(404, &[("content-type", "text/html")], "<h1>nope</h1>", 900);
    let outcomes = run(
        r#"
        [assert]
        status = [200, 201]
        duration_ms = { lt = 500 }

        [assert.headers]
        content-type = { contains = "json" }
        x-request-id = { exists = true }

        [assert.json]
        "$.id" = 1
        "#,
        &resp,
    );
    assert_eq!(
        failed(&outcomes),
        [
            "status equals [200,201]",
            "duration_ms < 500",
            "header content-type contains \"json\"",
            "header x-request-id exists",
            "json $.id equals 1",
        ]
    );
    let status = &outcomes[0];
    assert_eq!(status.actual.as_deref(), Some("404"));
    let missing = &outcomes[3];
    assert_eq!(missing.actual.as_deref(), Some("missing"));
    let json = &outcomes[4];
    assert!(
        json.message
            .as_deref()
            .unwrap()
            .starts_with("the body is not JSON"),
        "{json:?}"
    );
}

#[test]
fn type_mismatches_explain_themselves() {
    let resp = response(200, &[], USER, 1);
    let outcomes = run(
        r#"
        [assert.json]
        "$.name" = { gt = 1, length = 3 }
        "$.id" = { length = 1, contains = "x" }
        "$.missing" = { equals = 1 }
        "#,
        &resp,
    );
    let messages: Vec<(String, Option<String>)> = outcomes
        .iter()
        .map(|o| (format!("{} {}", o.subject, o.check), o.message.clone()))
        .collect();
    assert_eq!(
        messages,
        [
            ("json $.name length 3".into(), None),
            (
                "json $.name > 1".into(),
                Some("a string is not a number".into())
            ),
            (
                "json $.id contains \"x\"".into(),
                Some("cannot look inside a number".into())
            ),
            (
                "json $.id length 1".into(),
                Some("a number has no length".into())
            ),
            ("json $.missing equals 1".into(), Some("missing".into())),
        ]
    );
    assert!(!outcomes[0].passed, "`snap` has 4 characters");
}

#[test]
fn json_schema() {
    let dir = std::env::temp_dir().join(format!("snapreq-schema-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("user.json");
    std::fs::write(
        &file,
        r#"{"type": "object", "required": ["id", "name"], "properties": {"id": {"type": "integer"}, "name": {"type": "string", "minLength": 5}}}"#,
    )
    .unwrap();
    let mut s = AssertSpec {
        schema: Some(file.clone()),
        ..AssertSpec::default()
    };
    let a = Assertions::compile(&s).unwrap();

    let ok = a.evaluate(&response(200, &[], r#"{"id": 1, "name": "snapreq"}"#, 1));
    assert_eq!(ok.len(), 1);
    assert!(ok[0].passed && ok[0].subject == "schema user.json" && ok[0].check == "valid");

    let bad = a.evaluate(&response(200, &[], USER, 1));
    assert!(!bad[0].passed);
    let message = bad[0].message.as_deref().unwrap();
    assert!(message.starts_with("/name: "), "{message}");

    std::fs::write(&file, "{ not json").unwrap();
    assert!(Assertions::compile(&s).unwrap_err().contains("is not JSON"));
    s.schema = Some(dir.join("missing.json"));
    assert!(Assertions::compile(&s).unwrap_err().contains("cannot read"));
}

#[test]
fn invalid_paths_and_patterns_fail_before_sending() {
    let err = Assertions::compile(&spec("[assert.json]\n\"$.[\" = 1\n")).unwrap_err();
    assert!(
        err.starts_with("assert.json: invalid JSONPath `$.[`"),
        "{err}"
    );
    let err = Assertions::compile(&spec("[assert]\nbody = { matches = \"(\" }\n")).unwrap_err();
    assert!(
        err.starts_with("assert.body: invalid regular expression"),
        "{err}"
    );
    assert!(
        Assertions::compile(&AssertSpec::default())
            .unwrap()
            .is_empty()
    );
}
