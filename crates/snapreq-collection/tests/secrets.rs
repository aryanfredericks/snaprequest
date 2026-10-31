//! Secret values: sources, priority, storage and redaction.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::{Path, PathBuf};

use snapreq_collection::secrets::{SecretSource, store};
use snapreq_collection::{Collection, Environment, Scopes, Secrets, Variables, build_request};

/// A private copy of the fixture collection, so tests can write secret files.
fn copy_fixture(name: &str) -> PathBuf {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for entry in std::fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    let from = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic");
    let to = std::env::temp_dir().join(format!("snapreq-secrets-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&to);
    copy(&from, &to);
    to
}

fn scopes_with_secrets(c: &Collection, env: &str, env_vars: Vec<(String, String)>) -> Scopes {
    let environment = Environment::load(c, env).unwrap();
    let mut scopes = Scopes::for_request(
        c,
        "users/create-user",
        Some(&environment),
        &Variables::default(),
        &Variables::default(),
    );
    Secrets::load(c, Some(env), env_vars)
        .unwrap()
        .fill(&mut scopes);
    scopes
}

#[test]
fn stored_secrets_fill_variables_and_are_redacted() {
    let root = copy_fixture("store");
    let path = store(&root, Some("dev"), "token", "file-token-123").unwrap();
    assert_eq!(path, root.join(".secrets").join("dev.toml"));
    // The directory ignores itself in git.
    let ignore = std::fs::read_to_string(root.join(".secrets").join(".gitignore")).unwrap();
    assert!(ignore.lines().any(|l| l == "*"));

    let (c, _) = Collection::load(&root).unwrap();
    let scopes = scopes_with_secrets(&c, "dev", vec![]);
    let req = build_request(&c, "users/create-user", &scopes).unwrap();
    let auth = req
        .headers
        .iter()
        .find(|h| h.name == "Authorization")
        .unwrap();
    assert_eq!(auth.value, "Bearer file-token-123");
    assert_eq!(req.settings.secrets, ["file-token-123"]);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "secret files are private");
    }

    // `.secrets` is not mistaken for a folder.
    assert!(c.items.iter().all(|i| i.key() != ".secrets"));
}

#[test]
fn priority_env_var_then_environment_file_then_shared_file() {
    let root = copy_fixture("priority");
    store(&root, None, "token", "shared-value").unwrap();
    let (c, _) = Collection::load(&root).unwrap();

    let s = Secrets::load(&c, Some("dev"), vec![]).unwrap();
    let (value, source) = s.get("token").unwrap();
    assert_eq!(value, "shared-value");
    assert_eq!(
        source,
        SecretSource::File(root.join(".secrets").join("shared.toml"))
    );

    store(&root, Some("dev"), "token", "dev-value").unwrap();
    let s = Secrets::load(&c, Some("dev"), vec![]).unwrap();
    assert_eq!(s.get("token").unwrap().0, "dev-value");
    // Another environment only sees the shared value.
    assert_eq!(
        Secrets::load(&c, Some("prod"), vec![])
            .unwrap()
            .get("token")
            .unwrap()
            .0,
        "shared-value"
    );

    let env = vec![
        ("SNAPREQ_SECRET_TOKEN".to_owned(), "env-value".to_owned()),
        ("UNRELATED".to_owned(), "x".to_owned()),
    ];
    let s = Secrets::load(&c, Some("dev"), env).unwrap();
    let (value, source) = s.get("token").unwrap();
    assert_eq!(value, "env-value");
    assert_eq!(
        source.to_string(),
        "environment variable SNAPREQ_SECRET_TOKEN"
    );
}

#[test]
fn storing_again_replaces_the_value() {
    let root = copy_fixture("replace");
    store(&root, Some("dev"), "token", "one").unwrap();
    store(&root, Some("dev"), "other", "two").unwrap();
    store(&root, Some("dev"), "token", "three").unwrap();
    let text = std::fs::read_to_string(root.join(".secrets/dev.toml")).unwrap();
    assert!(
        text.ends_with("token = \"three\"\nother = \"two\"\n"),
        "{text}"
    );

    assert!(store(&root, None, "bad name", "x").is_err());
}

#[test]
fn missing_secret_still_reports_how_to_provide_it() {
    let root = copy_fixture("missing");
    let (c, _) = Collection::load(&root).unwrap();
    let scopes = scopes_with_secrets(&c, "dev", vec![]);
    let err = build_request(&c, "users/create-user", &scopes)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("secret variable `token` has no value"),
        "{err}"
    );
}
