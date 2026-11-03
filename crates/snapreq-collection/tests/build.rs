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

/// Writes `files` (relative path, contents) into a fresh collection directory.
fn temp_collection(tag: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snapreq-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        let path = dir.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    dir
}

fn build(
    dir: &Path,
    key: &str,
    vars: &[(&str, &str)],
) -> snapreq_collection::Result<snapreq_core::Request> {
    let (c, _) = Collection::load(dir).unwrap();
    let mut rt = Variables::default();
    for (k, v) in vars {
        rt.set(*k, *v);
    }
    let s = Scopes::for_request(&c, key, None, &Variables::default(), &rt);
    build_request(&c, key, &s)
}

#[test]
fn auth_is_inherited_overridden_and_switched_off() {
    use snapreq_core::model::{ApiKeyLocation, Auth};
    let dir = temp_collection(
        "auth",
        &[
            (
                "snapreq.toml",
                "format = 1\nname = \"A\"\n[auth]\ntype = \"bearer\"\ntoken = \"{{token}}\"\n",
            ),
            ("plain.toml", "url = \"http://a.test\"\n"),
            (
                "keys/folder.toml",
                "[auth]\ntype = \"api_key\"\nname = \"api_key\"\nvalue = \"{{key}}\"\nlocation = \"query\"\n",
            ),
            (
                "keys/inherits.toml",
                "url = \"http://a.test\"\n[auth]\ntype = \"inherit\"\n",
            ),
            (
                "keys/open.toml",
                "url = \"http://a.test\"\n[auth]\ntype = \"none\"\n",
            ),
            (
                "keys/digest.toml",
                "url = \"http://a.test\"\n[auth]\ntype = \"digest\"\nusername = \"{{user}}\"\npassword = \"pw\"\n",
            ),
            (
                "keys/aws.toml",
                "url = \"http://a.test\"\n[auth]\ntype = \"aws_sigv4\"\naccess_key = \"AK\"\nsecret_key = \"{{user}}\"\nregion = \"eu-west-1\"\nservice = \"s3\"\n",
            ),
            (
                "keys/basic.toml",
                "url = \"http://a.test\"\n[auth]\ntype = \"basic\"\nusername = \"{{user}}\"\n",
            ),
        ],
    );
    let vars = [("token", "tok"), ("key", "k"), ("user", "me")];

    assert_eq!(
        build(&dir, "plain", &vars).unwrap().auth,
        Auth::Bearer {
            token: "tok".into()
        }
    );
    assert_eq!(
        build(&dir, "keys/inherits", &vars).unwrap().auth,
        Auth::ApiKey {
            name: "api_key".into(),
            value: "k".into(),
            location: ApiKeyLocation::Query
        }
    );
    assert_eq!(build(&dir, "keys/open", &vars).unwrap().auth, Auth::None);
    assert_eq!(
        build(&dir, "keys/aws", &vars).unwrap().auth,
        Auth::AwsSigV4 {
            access_key: "AK".into(),
            secret_key: "me".into(),
            session_token: String::new(),
            region: "eu-west-1".into(),
            service: "s3".into()
        }
    );
    assert_eq!(
        build(&dir, "keys/digest", &vars).unwrap().auth,
        Auth::Digest {
            username: "me".into(),
            password: "pw".into()
        }
    );
    assert_eq!(
        build(&dir, "keys/basic", &vars).unwrap().auth,
        Auth::Basic {
            username: "me".into(),
            password: String::new()
        }
    );

    // Where each request's auth comes from.
    let (c, _) = Collection::load(&dir).unwrap();
    let source = |key: &str| {
        snapreq_collection::effective_auth(&c, key).map(|(spec, file)| {
            (
                spec.summary(),
                file.strip_prefix(&dir)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            )
        })
    };
    assert_eq!(
        source("plain"),
        Some(("bearer".into(), "snapreq.toml".into()))
    );
    assert_eq!(
        source("keys/inherits"),
        Some(("api_key query api_key".into(), "keys/folder.toml".into()))
    );
    assert_eq!(
        source("keys/open"),
        Some(("none".into(), "keys/open.toml".into()))
    );

    // Overridden auth is not interpolated: `token` and `key` may be undefined there.
    assert!(build(&dir, "keys/basic", &[("user", "me")]).is_ok());
    let err = build(&dir, "keys/inherits", &[]).unwrap_err().to_string();
    assert!(
        err.contains("folder.toml: auth.value: variable `key` is not defined"),
        "{err}"
    );
}

#[test]
fn secret_auth_values_are_marked_for_redaction() {
    let dir = temp_collection(
        "auth-secret",
        &[
            (
                "snapreq.toml",
                "format = 1\nname = \"S\"\n[vars]\npass = { secret = true }\n",
            ),
            (
                "r.toml",
                "url = \"http://a.test\"\n[auth]\ntype = \"basic\"\nusername = \"u\"\npassword = \"{{pass}}\"\n",
            ),
        ],
    );
    let err = build(&dir, "r", &[]).unwrap_err().to_string();
    assert!(
        err.contains("r.toml: auth.password: secret variable `pass`"),
        "{err}"
    );

    // Auth credentials are always hidden, wherever their values come from.
    let req = build(&dir, "r", &[("pass", "hunter2")]).unwrap();
    let secrets = req.secrets();
    assert!(secrets.contains(&"hunter2".to_owned()), "{secrets:?}");
    assert!(secrets.contains(&"dTpodW50ZXIy".to_owned()), "{secrets:?}");
}

#[test]
fn bad_auth_tables_are_errors() {
    for (text, needle) in [
        ("[auth]\ntype = \"oauth9\"\n", "unknown variant `oauth9`"),
        (
            "[auth]\ntype = \"bearer\"\ntokn = \"x\"\n",
            "unknown field `tokn`",
        ),
        ("[auth]\ntype = \"bearer\"\n", "missing field `token`"),
    ] {
        let dir = temp_collection(
            "auth-bad",
            &[
                ("snapreq.toml", "format = 1\nname = \"B\"\n"),
                ("r.toml", &format!("url = \"http://a.test\"\n{text}")),
            ],
        );
        let err = Collection::load(&dir).unwrap_err().to_string();
        assert!(err.contains(needle), "{err}");
    }
    let dir = temp_collection(
        "auth-empty-name",
        &[
            ("snapreq.toml", "format = 1\nname = \"B\"\n"),
            (
                "r.toml",
                "url = \"http://a.test\"\n[auth]\ntype = \"api_key\"\nname = \" \"\nvalue = \"v\"\n",
            ),
        ],
    );
    let err = build(&dir, "r", &[]).unwrap_err().to_string();
    assert!(err.contains("auth.name: must not be empty"), "{err}");
}

#[test]
fn oauth2_auth_tables() {
    use snapreq_core::model::{Auth, ClientAuth, OAuth2Grant};
    let dir = temp_collection(
        "oauth2",
        &[
            (
                "snapreq.toml",
                r#"format = 1
name = "O"
[vars]
issuer = "https://login.test"
secret = { secret = true }

[auth]
type = "oauth2"
grant = "client_credentials"
token_url = "{{issuer}}/token"
client_id = "app"
client_secret = "{{secret}}"
scope = "read write"
client_auth = "body"

[auth.params]
audience = "https://api.test"
"#,
            ),
            ("r.toml", "url = \"http://a.test\"\n"),
            (
                "code.toml",
                "url = \"http://a.test\"\n[auth]\ntype = \"oauth2\"\ngrant = \"authorization_code\"\ntoken_url = \"t\"\nclient_id = \"c\"\n",
            ),
        ],
    );
    let req = build(&dir, "r", &[("secret", "s3")]).unwrap();
    let Auth::OAuth2(o) = &req.auth else {
        panic!("oauth2 expected, got {:?}", req.auth)
    };
    assert_eq!(o.grant, OAuth2Grant::ClientCredentials);
    assert_eq!(o.token_url, "https://login.test/token");
    assert_eq!(o.client_secret, "s3");
    assert_eq!(o.client_auth, ClientAuth::Body);
    assert_eq!(
        (o.params[0].name.as_str(), o.params[0].value.as_str()),
        ("audience", "https://api.test")
    );
    assert!(req.secrets().contains(&"s3".to_owned()));

    let err = build(&dir, "code", &[("secret", "s3")])
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("code.toml: auth: the authorization_code grant needs `auth_url`"),
        "{err}"
    );

    // Saving keeps the layout readable.
    let (c, _) = Collection::load(&dir).unwrap();
    let text = snapreq_collection::save::to_toml(&c.file).unwrap();
    assert!(
        text.contains("[auth]\ntype = \"oauth2\"\ngrant = \"client_credentials\"\ntoken_url = \"{{issuer}}/token\""),
        "{text}"
    );
    assert!(
        text.contains("[auth.params]\naudience = \"https://api.test\"\n"),
        "{text}"
    );
    let back: snapreq_collection::CollectionFile = toml::from_str(&text).unwrap();
    assert_eq!(back, c.file);
}

#[test]
fn assertions_are_interpolated_and_round_trip() {
    let dir = temp_collection(
        "assert",
        &[
            (
                "snapreq.toml",
                "format = 1\nname = \"A\"\n[vars]\nname = \"snap\"\n",
            ),
            (
                "users/get.toml",
                r#"url = "http://a.test"

[assert]
status = "2xx"
schema = "schemas/user.json"

[assert.headers]
content-type = { contains = "json" }

[assert.json]
"$.name" = "{{name}}"
"$.id" = { type = "integer", gt = 0 }
"#,
            ),
        ],
    );
    let (c, _) = Collection::load(&dir).unwrap();
    let s = Scopes::for_request(
        &c,
        "users/get",
        None,
        &Variables::default(),
        &Variables::default(),
    );
    let a = snapreq_collection::build_assertions(&c, "users/get", &s).unwrap();
    assert_eq!(a.json.0[0].1, snapreq_collection::Check::equals("snap"));
    assert_eq!(a.schema, Some(dir.join("users").join("schemas/user.json")));

    let file = &c.find("users/get").unwrap().file;
    let text = snapreq_collection::save::to_toml(file).unwrap();
    assert!(
        text.contains("[assert]\nstatus = \"2xx\"\nschema = \"schemas/user.json\"\n"),
        "{text}"
    );
    assert!(
        text.contains(
            "[assert.json]\n\"$.name\" = \"{{name}}\"\n\"$.id\" = { type = \"integer\", gt = 0 }\n"
        ),
        "{text}"
    );
    let back: snapreq_collection::RequestFile = toml::from_str(&text).unwrap();
    assert_eq!(&back, file);

    let s = Scopes::for_request(
        &c,
        "users/get",
        None,
        &Variables::default(),
        &Variables::default(),
    );
    std::fs::write(
        dir.join("users/get.toml"),
        "url = \"http://a.test\"\n[assert.json]\n\"$.x\" = \"{{missing}}\"\n",
    )
    .unwrap();
    let (c, _) = Collection::load(&dir).unwrap();
    let err = snapreq_collection::build_assertions(&c, "users/get", &s)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("get.toml: assert.json.$.x: variable `missing`"),
        "{err}"
    );
}
