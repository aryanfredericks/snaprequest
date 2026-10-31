//! Building core requests from collection requests.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::{Path, PathBuf};
use std::time::Duration;

use snapreq_collection::{Collection, Environment, Scopes, Variables, build_request};
use snapreq_core::model::{Body, PartValue};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic")
}

fn scopes(c: &Collection, key: &str, env: Option<&str>, runtime: &[(&str, &str)]) -> Scopes {
    let env = env.map(|e| Environment::load(c, e).unwrap());
    let mut rt = Variables::default();
    for (k, v) in runtime {
        rt.set(*k, *v);
    }
    Scopes::for_request(c, key, env.as_ref(), &Variables::default(), &rt)
}

#[test]
fn placeholders_are_resolved_from_the_environment() {
    let (c, _) = Collection::load(root()).unwrap();
    let s = scopes(&c, "users/list-users", Some("prod"), &[]);
    let req = build_request(&c, "users/list-users", &s).unwrap();

    assert_eq!(req.method, http::Method::GET);
    assert_eq!(req.url, "https://api.example.com/users");
    let query: Vec<(&str, &str, bool)> = req
        .query
        .iter()
        .map(|q| (q.name.as_str(), q.value.as_str(), q.enabled))
        .collect();
    assert_eq!(
        query,
        [
            ("page", "2", true),
            ("tag", "a", true),
            ("tag", "b", true),
            ("draft", "true", false)
        ]
    );
    assert!(req.settings.secrets.is_empty());
}

#[test]
fn secret_variables_need_a_value_and_are_marked_for_redaction() {
    let (c, _) = Collection::load(root()).unwrap();

    // `token` is a secret without a value in the dev environment.
    let s = scopes(&c, "users/create-user", Some("dev"), &[]);
    let err = build_request(&c, "users/create-user", &s)
        .unwrap_err()
        .to_string();
    assert!(err.contains("create-user.toml"), "{err}");
    assert!(
        err.contains("headers.Authorization: secret variable `token` has no value"),
        "{err}"
    );

    let s = scopes(
        &c,
        "users/create-user",
        Some("dev"),
        &[("token", "abc-123-secret")],
    );
    let req = build_request(&c, "users/create-user", &s).unwrap();
    assert_eq!(req.url, "http://localhost:8080/users");
    assert_eq!(req.headers[0].value, "Bearer abc-123-secret");
    // A runtime value is not itself marked secret; secrets come from secret variables.
    assert!(req.settings.secrets.is_empty());
    assert!(!req.settings.follow_redirects);
    let Body::Json { content } = &req.body else {
        panic!("expected JSON body");
    };
    assert!(content.contains("\"name\": \"snap\""));
}

#[test]
fn undefined_variables_name_the_field_and_scopes() {
    let (c, _) = Collection::load(root()).unwrap();
    let s = Scopes::for_request(
        &c,
        "health",
        None,
        &Variables::default(),
        &Variables::default(),
    );
    // Collection defaults define base_url, so this works without an environment.
    let req = build_request(&c, "health", &s).unwrap();
    assert_eq!(req.url, "http://localhost:3000/health");

    let mut no_defaults = c.clone();
    no_defaults.file.vars = Variables::default();
    let s = Scopes::for_request(
        &no_defaults,
        "health",
        None,
        &Variables::default(),
        &Variables::default(),
    );
    let err = build_request(&no_defaults, "health", &s)
        .unwrap_err()
        .to_string();
    assert!(
        err.ends_with("url: variable `base_url` is not defined (searched: runtime, request, collection, global)"),
        "{err}"
    );
}

#[test]
fn relative_paths_resolve_against_the_request_file() {
    let (c, _) = Collection::load(root()).unwrap();
    let s = scopes(&c, "users/upload-avatar", None, &[]);
    let req = build_request(&c, "users/upload-avatar", &s).unwrap();
    let Body::Multipart { parts } = &req.body else {
        panic!("expected multipart");
    };
    let PartValue::File {
        path, content_type, ..
    } = &parts[1].value
    else {
        panic!("expected a file part");
    };
    assert_eq!(*path, root().join("users").join("avatar.png"));
    assert!(path.is_file());
    assert_eq!(content_type.as_deref(), Some("image/png"));
    assert_eq!(
        req.settings.timeout,
        Some(Duration::from_secs(30)),
        "core default until P3.5 inheritance"
    );
}

#[test]
fn unknown_requests() {
    let (c, _) = Collection::load(root()).unwrap();
    let s = Scopes::default();
    assert!(
        build_request(&c, "nope", &s)
            .unwrap_err()
            .to_string()
            .contains("no request `nope`")
    );
}
