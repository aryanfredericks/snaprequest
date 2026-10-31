//! Variable scopes and environments.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::{Path, PathBuf};

use snapreq_collection::save::save_environment;
use snapreq_collection::{Collection, Environment, Scope, Scopes, Variables};

fn fixture() -> Collection {
    let root: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/basic");
    Collection::load(root).unwrap().0
}

fn value<'a>(scopes: &'a Scopes, name: &str) -> (Option<&'a str>, String) {
    let (var, scope) = scopes.lookup(name).unwrap();
    (var.value.as_deref(), scope.to_string())
}

#[test]
fn environments_are_listed_and_loaded() {
    let c = fixture();
    assert_eq!(Environment::list(&c).unwrap(), ["dev", "prod"]);

    let dev = Environment::load(&c, "dev").unwrap();
    assert_eq!(
        dev.vars.get("base_url").unwrap().value.as_deref(),
        Some("http://localhost:8080")
    );
    assert!(dev.vars.get("token").unwrap().secret);

    let err = Environment::load(&c, "staging").unwrap_err().to_string();
    assert!(
        err.contains("no environment `staging` (available: dev, prod)"),
        "{err}"
    );
}

#[test]
fn precedence_from_runtime_down_to_global() {
    let c = fixture();
    let dev = Environment::load(&c, "dev").unwrap();
    let mut globals = Variables::default();
    globals.set("level", "global");
    globals.set("only_global", "g");
    let mut runtime = Variables::default();
    runtime.set("level", "runtime");

    let all = Scopes::for_request(&c, "users/create-user", Some(&dev), &globals, &runtime);
    assert_eq!(value(&all, "level"), (Some("runtime"), "runtime".into()));

    let no_runtime = Scopes::for_request(
        &c,
        "users/create-user",
        Some(&dev),
        &globals,
        &Variables::default(),
    );
    assert_eq!(
        value(&no_runtime, "level"),
        (Some("request"), "request".into())
    );

    // list-users has no request variables: the folder wins.
    let list = Scopes::for_request(
        &c,
        "users/list-users",
        Some(&dev),
        &globals,
        &Variables::default(),
    );
    assert_eq!(
        value(&list, "level"),
        (Some("folder"), "folder `users`".into())
    );
    assert_eq!(
        value(&list, "section"),
        (Some("users"), "folder `users`".into())
    );

    // Outside the folder: the environment overrides collection defaults.
    let health = Scopes::for_request(&c, "health", Some(&dev), &globals, &Variables::default());
    assert_eq!(
        value(&health, "level"),
        (Some("environment"), "environment `dev`".into())
    );
    assert_eq!(
        value(&health, "base_url"),
        (Some("http://localhost:8080"), "environment `dev`".into())
    );
    assert_eq!(
        value(&health, "api_version"),
        (Some("v1"), "collection".into())
    );
    assert_eq!(value(&health, "only_global"), (Some("g"), "global".into()));
    assert!(health.lookup("nope").is_none());

    // Without an environment, collection defaults apply.
    let plain = Scopes::for_request(&c, "health", None, &globals, &Variables::default());
    assert_eq!(
        value(&plain, "base_url"),
        (Some("http://localhost:3000"), "collection".into())
    );
    assert_eq!(
        value(&plain, "level"),
        (Some("collection"), "collection".into())
    );
}

#[test]
fn visible_variables_and_runtime_updates() {
    let c = fixture();
    let mut scopes = Scopes::for_request(
        &c,
        "users/list-users",
        None,
        &Variables::default(),
        &Variables::default(),
    );
    let names: Vec<(String, String)> = scopes
        .visible()
        .into_iter()
        .map(|(v, s)| (v.name.clone(), s.to_string()))
        .collect();
    assert_eq!(
        names,
        [
            ("level".into(), "folder `users`".into()),
            ("section".into(), "folder `users`".into()),
            ("base_url".into(), "collection".into()),
            ("api_version".into(), "collection".into()),
        ]
    );

    scopes.set_runtime("user_id", "42");
    assert_eq!(scopes.lookup("user_id").unwrap().1, &Scope::Runtime);
}

#[test]
fn environments_save_without_secret_values() {
    let c = fixture();
    let mut dev = Environment::load(&c, "dev").unwrap();
    dev.vars
        .0
        .iter_mut()
        .find(|v| v.name == "token")
        .unwrap()
        .value = Some("loaded-from-elsewhere".into());

    let dir = std::env::temp_dir().join(format!("snapreq-env-{}", std::process::id()));
    let path = dir.join("dev.toml");
    save_environment(&path, &dev.vars).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(
        text,
        "base_url = \"http://localhost:8080\"\nlevel = \"environment\"\ntoken = { secret = true }\n"
    );
}
