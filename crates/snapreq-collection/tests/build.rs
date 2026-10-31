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
    let auth = req
        .headers
        .iter()
        .find(|h| h.name == "Authorization")
        .unwrap();
    assert_eq!(auth.value, "Bearer abc-123-secret");
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
    // Inherited from `snapreq.toml` (`timeout_ms = 10000`).
    assert_eq!(req.settings.timeout, Some(Duration::from_secs(10)));
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

#[test]
fn headers_are_inherited_from_collection_and_folders() {
    let (c, _) = Collection::load(root()).unwrap();
    let s = scopes(&c, "users/create-user", Some("dev"), &[("token", "t")]);
    let req = build_request(&c, "users/create-user", &s).unwrap();
    let names: Vec<&str> = req.headers.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["Accept", "X-Section", "Authorization"]);
    // Request settings override the collection's; untouched ones are inherited.
    assert!(!req.settings.follow_redirects);
    assert_eq!(req.settings.timeout, Some(Duration::from_secs(10)));
}

#[test]
fn overriding_and_switching_off_inherited_headers() {
    let dir = std::env::temp_dir().join(format!("snapreq-inherit-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("admin/certs")).unwrap();
    std::fs::write(
        dir.join("snapreq.toml"),
        "format = 1\nname = \"I\"\n[headers]\nAccept = \"application/json\"\nX-Trace = \"on\"\nX-Env = \"base\"\n[settings]\ntimeout_ms = 1000\nca_cert = \"root-ca.pem\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("admin/folder.toml"),
        "[headers]\nx-env = \"admin\"\n[settings]\ntimeout_ms = 2000\nclient_cert = \"certs/me.crt\"\nclient_key = \"certs/me.key\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("admin/users.toml"),
        "url = \"http://a.test\"\n[headers]\nX-Trace = { value = \"on\", enabled = false }\n[settings]\ntimeout_ms = 0\n",
    )
    .unwrap();

    let (c, _) = Collection::load(&dir).unwrap();
    let s = Scopes::for_request(
        &c,
        "admin/users",
        None,
        &Variables::default(),
        &Variables::default(),
    );
    let req = build_request(&c, "admin/users", &s).unwrap();

    let headers: Vec<(&str, &str, bool)> = req
        .headers
        .iter()
        .map(|h| (h.name.as_str(), h.value.as_str(), h.enabled))
        .collect();
    assert_eq!(
        headers,
        [
            ("Accept", "application/json", true),
            // The folder replaces X-Env (case-insensitive match).
            ("x-env", "admin", true),
            // The request switches X-Trace off.
            ("X-Trace", "on", false),
        ]
    );
    // `0` means no limit and overrides the inherited 2000 ms.
    assert_eq!(req.settings.timeout, None);
    // Paths resolve against the file that set them.
    assert_eq!(req.settings.tls.ca_cert, Some(dir.join("root-ca.pem")));
    assert_eq!(
        req.settings.tls.client_cert.unwrap().cert,
        dir.join("admin").join("certs/me.crt")
    );
}

#[test]
fn errors_in_inherited_values_name_the_file_that_has_them() {
    let dir = std::env::temp_dir().join(format!("snapreq-inherit-err-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("f")).unwrap();
    std::fs::write(dir.join("snapreq.toml"), "format = 1\nname = \"E\"\n").unwrap();
    std::fs::write(
        dir.join("f/folder.toml"),
        "[headers]\nX-Key = \"{{missing}}\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("f/r.toml"), "url = \"http://a.test\"\n").unwrap();

    let (c, _) = Collection::load(&dir).unwrap();
    let s = Scopes::for_request(
        &c,
        "f/r",
        None,
        &Variables::default(),
        &Variables::default(),
    );
    let err = build_request(&c, "f/r", &s).unwrap_err().to_string();
    assert!(
        err.contains("folder.toml: headers.X-Key: variable `missing`"),
        "{err}"
    );
}
