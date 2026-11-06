//! Importing every supported format (PLAN P9.1): the written collection loads, keeps the
//! source's structure, and never holds a credential in a collection file.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::{Path, PathBuf};

use snapreq_collection::{AuthSpec, BodySpec, Collection, Environment};
use snapreq_interop::{Format, Options, Summary, add_postman_environment, detect, import};

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .unwrap()
}

fn target(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("snapreq-import-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Imports a fixture, writes it and loads it back.
fn imported(name: &str, tag: &str) -> (Collection, Summary, PathBuf) {
    let imp = import(&fixture(name), &Options::default()).unwrap();
    let dir = target(tag);
    let summary = imp.write(&dir, None).unwrap();
    let (collection, warnings) = Collection::load(&dir).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    (collection, summary, dir)
}

/// Every collection file (not `.secrets`) as one string.
fn collection_text(dir: &Path) -> String {
    let mut out = String::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let e = e.unwrap();
            if e.file_name() == ".secrets" {
                continue;
            }
            if e.file_type().unwrap().is_dir() {
                stack.push(e.path());
            } else {
                out.push_str(&std::fs::read_to_string(e.path()).unwrap());
            }
        }
    }
    out
}

fn shared_secrets(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(".secrets/shared.toml")).unwrap_or_default()
}

#[test]
fn formats_are_detected() {
    for (file, format) in [
        ("commands.curl", Format::Curl),
        ("postman-shop.json", Format::Postman),
        ("postman-newman-sample.json", Format::Postman),
        (
            "postman-dev.postman_environment.json",
            Format::PostmanEnvironment,
        ),
        ("openapi-petstore.yaml", Format::OpenApi),
        ("insomnia-v4.json", Format::Insomnia),
        ("insomnia-v5.yaml", Format::Insomnia),
        ("recording.har", Format::Har),
        ("smoke.hurl", Format::Hurl),
    ] {
        assert_eq!(detect(&fixture(file)).unwrap(), format, "{file}");
    }
    let swagger = detect("{\"swagger\": \"2.0\", \"paths\": {}}").unwrap_err();
    assert!(swagger.to_string().contains("OpenAPI 3"), "{swagger}");
    assert!(detect("hello: world").is_err());
    assert!(detect("{not json").is_err());
}

#[test]
fn postman_collection_with_environment() {
    let mut imp = import(&fixture("postman-shop.json"), &Options::default()).unwrap();
    let env = add_postman_environment(&mut imp, &fixture("postman-dev.postman_environment.json"))
        .unwrap();
    assert_eq!(env, "dev-env");
    let dir = target("postman");
    let summary = imp.write(&dir, None).unwrap();
    assert_eq!(
        (summary.requests, summary.folders, summary.flows),
        (7, 2, 0)
    );
    assert_eq!(summary.secrets, 4);
    let (c, warnings) = Collection::load(&dir).unwrap();
    assert!(warnings.is_empty());
    assert_eq!(c.file.name, "Shop API");
    assert_eq!(c.file.order, ["users", "public"]);
    assert_eq!(
        c.file.auth,
        Some(AuthSpec::Bearer {
            token: "{{token}}".into()
        })
    );

    let get = c.find("users/get-user").unwrap();
    assert_eq!(get.file.url, "{{base_url}}/users/{{id}}");
    assert_eq!(
        get.file.vars.get("id").unwrap().value.as_deref(),
        Some("42")
    );
    let list = c.find("users/list-users").unwrap();
    let query: Vec<_> = list
        .file
        .query
        .0
        .iter()
        .map(|q| (q.name.as_str(), q.value.as_str(), q.enabled))
        .collect();
    assert_eq!(
        query,
        [
            ("page", "1", true),
            ("size", "{{limit}}", true),
            ("draft", "true", false)
        ]
    );
    let create = c.find("users/create-user").unwrap();
    match &create.file.body {
        Some(BodySpec::Json {
            content: Some(text),
            ..
        }) => assert!(text.contains("{{$uuid}}"), "{text}"),
        other => panic!("{other:?}"),
    }
    assert!(
        create.file.headers.is_empty(),
        "Content-Type comes from the body"
    );
    assert!(matches!(
        c.find("public/login").unwrap().file.body,
        Some(BodySpec::Form { .. })
    ));
    let upload = c.find("public/upload-avatar").unwrap();
    assert!(
        upload.file.headers.is_empty(),
        "no stale multipart boundary"
    );
    let feed = c.find("public/partner-feed").unwrap();
    assert!(
        matches!(&feed.file.auth, Some(AuthSpec::ApiKey { name, value, .. })
        if name == "X-Partner-Key" && value == "{{x_partner_key}}")
    );

    // The environment, with its secret values kept aside.
    let dev = Environment::load(&c, "dev-env").unwrap();
    assert_eq!(
        dev.vars.get("base_url").unwrap().value.as_deref(),
        Some("http://dev.local")
    );
    assert!(dev.vars.get("token").unwrap().secret);
    assert!(
        dev.vars.get("unused").is_none(),
        "disabled values are skipped"
    );
    let dev_secrets = std::fs::read_to_string(dir.join(".secrets/dev-env.toml")).unwrap();
    assert!(dev_secrets.contains("dev-token-value-123"));

    // No credential reached a collection file.
    let text = collection_text(&dir);
    for secret in [
        "dev-token-value-123",
        "dev-password-456",
        "pk_live_1234567890",
        "abc123secret",
    ] {
        assert!(!text.contains(secret), "{secret} leaked");
    }
    assert!(shared_secrets(&dir).contains("pk_live_1234567890"));

    // Scripts are translated; one that cannot be is kept as a comment.
    let list_script = list.file.script.test.as_deref().unwrap();
    assert_eq!(
        list_script.trim(),
        "snapreq.test('ok', () => snapreq.expect(snapreq.response.status).to.equal(200));"
    );
    let login = c.find("public/login").unwrap();
    assert!(
        login
            .file
            .script
            .pre_request
            .as_deref()
            .unwrap()
            .lines()
            .all(|l| l.starts_with("//")),
        "{:?}",
        login.file.script
    );
    for needle in [
        "page size",
        "`Login`: the pre-request script could not be translated",
        "{{$randomColor}}",
        "me.png",
    ] {
        assert!(
            summary.warnings.iter().any(|w| w.contains(needle)),
            "{needle}: {:?}",
            summary.warnings
        );
    }
}

#[test]
fn the_public_newman_sample_collection() {
    let (c, summary, _) = imported("postman-newman-sample.json", "newman");
    assert_eq!(summary.requests, 3);
    let get = c.find("a-simple-get-request").unwrap();
    assert_eq!(get.file.url, "https://postman-echo.com/get");
    assert_eq!(get.file.query.0[0].value, "newman-sample-github-collection");
    assert!(matches!(
        &c.find("a-simple-post-request").unwrap().file.body,
        Some(BodySpec::Text { content_type: Some(ct), .. }) if ct == "text/plain"
    ));
}

#[test]
fn openapi_operations_servers_and_security() {
    let (c, summary, dir) = imported("openapi-petstore.yaml", "openapi");
    assert_eq!((summary.requests, summary.folders), (7, 2));
    assert_eq!(summary.environments, ["production", "local"]);
    assert_eq!(c.file.order, ["pets", "store", "health"]);
    assert_eq!(
        c.file.vars.get("base_url").unwrap().value.as_deref(),
        Some("https://petstore.example.com/v1")
    );
    assert!(c.file.vars.get("token").unwrap().secret);
    assert_eq!(summary.secrets, 0, "declared, not given");

    let list = c.find("pets/list-pets").unwrap();
    let query: Vec<_> = list
        .file
        .query
        .0
        .iter()
        .map(|q| (q.name.as_str(), q.value.as_str(), q.enabled))
        .collect();
    assert_eq!(
        query,
        [("limit", "20", false), ("status", "available", true)]
    );
    assert_eq!(list.file.headers.0[0].name, "X-Trace-Id");

    let get = c.find("pets/getpet").unwrap();
    assert_eq!(get.file.url, "{{base_url}}/pets/{{petId}}");
    assert_eq!(
        get.file.vars.get("petId").unwrap().value.as_deref(),
        Some("7")
    );
    assert!(get.file.auth.is_none(), "inherits the global bearer auth");
    assert!(matches!(
        c.find("pets/delete-a-pet").unwrap().file.auth,
        Some(AuthSpec::ApiKey { .. })
    ));
    assert_eq!(c.find("health").unwrap().file.auth, Some(AuthSpec::None));

    let create = c.find("pets/create-a-pet").unwrap();
    let Some(BodySpec::Json {
        content: Some(body),
        ..
    }) = &create.file.body
    else {
        panic!()
    };
    let json: serde_json::Value = serde_json::from_str(&body.replace("\\{{", "{{")).unwrap();
    assert_eq!(json["born"], "2024-01-01");
    assert!(
        json.get("id").is_none(),
        "read-only properties are left out"
    );
    assert_eq!(json["owner"]["pets"][0]["owner"], serde_json::Value::Null);
    assert!(body.contains("\\{{not a var}}"), "examples are literal");

    assert!(matches!(
        &c.find("pets/upload-a-photo").unwrap().file.body,
        Some(BodySpec::Multipart { parts }) if parts.len() == 2
    ));
    assert!(collection_text(&dir).contains("token = { secret = true }"));
}

#[test]
fn insomnia_v4_and_v5() {
    let (c, summary, dir) = imported("insomnia-v4.json", "insomnia4");
    assert_eq!(c.file.name, "Weather");
    assert_eq!(summary.environments, ["production"]);
    assert_eq!(c.file.order, ["forecasts", "login"]);
    let folder = &c.items[0];
    let snapreq_collection::Item::Folder(f) = folder else {
        panic!()
    };
    assert_eq!(f.file.order, ["hourly", "daily"], "Insomnia's sort order");
    let daily = c.find("forecasts/daily").unwrap();
    assert_eq!(daily.file.url, "{{host}}/{{api.version}}/daily");
    assert_eq!(daily.file.headers.0[0].value, "{{$uuid}}");
    assert!(
        c.file.vars.get("api.version").is_some(),
        "nested data is flattened"
    );
    let text = collection_text(&dir);
    assert!(!text.contains("wx-live-token-999") && !text.contains("hunter2-insomnia"));
    assert!(shared_secrets(&dir).contains("hunter2-insomnia"));
    assert!(summary.warnings.iter().any(|w| w.contains("gRPC")));

    let (c, _, _) = imported("insomnia-v5.yaml", "insomnia5");
    assert_eq!(c.file.name, "Notes");
    let add = c.find("notes/add-note").unwrap();
    assert_eq!(add.file.url, "{{base}}/notes");
    assert!(
        matches!(&add.file.auth, Some(AuthSpec::ApiKey { value, .. }) if value == "{{notes_key}}")
    );
    let Some(snapreq_collection::Item::Folder(f)) = c.items.first() else {
        panic!()
    };
    assert_eq!(f.file.order, ["add-note", "list-notes"]);
}

#[test]
fn har_recordings() {
    let (c, summary, dir) = imported("recording.har", "har");
    assert_eq!(summary.requests, 2, "the page and the image are skipped");
    assert_eq!(
        c.file.vars.get("base_url").unwrap().value.as_deref(),
        Some("https://app.example.com")
    );
    let get = c.find("get-api-items").unwrap();
    assert_eq!(get.file.url, "{{base_url}}/api/items");
    let names: Vec<_> = get.file.headers.0.iter().map(|h| h.name.as_str()).collect();
    assert_eq!(names, ["accept", "cookie"]);
    let post = c.find("post-api-items").unwrap();
    assert!(post.file.headers.is_empty());
    match &post.file.body {
        Some(BodySpec::Json {
            content: Some(t), ..
        }) => assert_eq!(t, r#"{"name":"\{{literal}}"}"#),
        other => panic!("{other:?}"),
    }
    let text = collection_text(&dir);
    assert!(!text.contains("recorded.token") && !text.contains("recorded-session-cookie"));
    assert_eq!(
        get.file.auth,
        Some(AuthSpec::Bearer {
            token: "{{token}}".into()
        }),
        "the same token is one variable"
    );
    assert_eq!(post.file.auth, get.file.auth);
}

#[test]
fn hurl_files_become_flows() {
    let (c, summary, dir) = imported("smoke.hurl", "hurl");
    assert_eq!((summary.requests, summary.flows), (0, 1));
    let flow = &c.flows[0].file;
    assert_eq!(flow.steps.len(), 3);
    let login = &flow.steps[0];
    assert_eq!(login.method.as_deref(), Some("POST"));
    assert!(matches!(login.body, Some(BodySpec::Json { .. })));
    assert!(
        login.capture.0[0].1.secret,
        "`redact` makes a secret capture"
    );
    assert_eq!(login.capture.0[1].1.cookie.as_deref(), Some("sid"));
    assert_eq!(login.assert.json.0[0].1.exists, Some(true));
    assert!(login.assert.duration_ms.is_some());
    let me = &flow.steps[1];
    assert_eq!(
        me.retry.as_ref().map(|r| (r.max, r.every_ms)),
        Some((3, 100))
    );
    assert_eq!(me.assert.json.0.len(), 4);
    assert_eq!(me.assert.json.0[1].1.length, Some(2));
    let delete = &flow.steps[2];
    // The file's own `{{password}}` is not taken over by the [BasicAuth] secret.
    assert!(
        matches!(&delete.auth, Some(AuthSpec::Basic { password, .. }) if password == "{{password_2}}")
    );
    assert!(!collection_text(&dir).contains("s3cret-basic"));
    assert!(summary.warnings.iter().any(|w| w.contains("xpath")));
    assert!(
        summary
            .warnings
            .iter()
            .any(|w| w.contains("not defined anywhere: base_url, password"))
    );
}

#[test]
fn curl_commands() {
    let (c, summary, dir) = imported("commands.curl", "curl");
    assert_eq!(summary.requests, 3);
    let post = c.find("post-v1-users").unwrap();
    assert_eq!(post.file.method(), "POST");
    assert_eq!(
        post.file.url,
        "https://api.example.com/v1/users?notify=true"
    );
    assert_eq!(post.file.settings.timeout_ms, Some(5000));
    assert_eq!(post.file.settings.follow_redirects, Some(true));
    let upload = c.find("post-v1-upload").unwrap();
    assert!(
        matches!(&upload.file.auth, Some(AuthSpec::Basic { username, .. }) if username == "ana")
    );
    let search = c.find("get-v1-search").unwrap();
    assert_eq!(
        search.file.url,
        "https://api.example.com/v1/search?q=hello+world&limit=3"
    );
    assert_eq!(search.file.settings.verify_tls, Some(false));
    let text = collection_text(&dir);
    assert!(!text.contains("sk_live_curl_token") && !text.contains("curl-pass"));
}

#[test]
fn single_curl_commands_from_a_clipboard() {
    for (cmd, check) in [
        (
            "curl 'https://a.test/x' -H 'accept: */*' --json '{\"a\":1}'",
            "json",
        ),
        ("curl -XPUT -d 'a=1' -d 'b=2' a.test/form", "form"),
        ("curl -I https://a.test", "head"),
        ("curl.exe \"https://a.test\" ^\n -H \"X-A: 1\"", "windows"),
        ("curl --oauth2-bearer tok https://a.test", "bearer"),
    ] {
        let imp = import(cmd, &Options::default()).unwrap();
        let snapreq_interop::Node::Request(r) = &imp.items[0] else {
            panic!()
        };
        match check {
            "json" => {
                assert!(matches!(r.body, Some(BodySpec::Json { .. })) && r.method() == "POST")
            }
            "form" => {
                assert_eq!(r.method(), "PUT");
                assert_eq!(r.url, "http://a.test/form");
                let Some(BodySpec::Form { fields }) = &r.body else {
                    panic!()
                };
                assert_eq!(fields.0.len(), 2);
            }
            "head" => assert_eq!(r.method(), "HEAD"),
            "windows" => assert_eq!(r.headers.0[0].name, "X-A"),
            "bearer" => {
                assert!(matches!(&r.auth, Some(AuthSpec::Bearer { token }) if token == "{{token}}"))
            }
            _ => unreachable!(),
        }
    }
    assert!(
        import("curl -H 'X: 1'", &Options::default()).is_err(),
        "no URL"
    );
}

#[test]
fn writing_refuses_a_non_empty_directory_and_names_can_be_overridden() {
    let imp = import(
        "curl https://a.test",
        &Options {
            format: Some(Format::Curl),
            name: Some("Scratch".into()),
        },
    )
    .unwrap();
    assert_eq!(imp.collection.name, "Scratch");
    let dir = target("exists");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("keep.txt"), "x").unwrap();
    let err = imp.write(&dir, None).unwrap_err();
    assert!(err.to_string().contains("not empty"), "{err}");
    assert!(dir.join("keep.txt").is_file());
}

#[test]
fn sealed_secrets_with_a_key() {
    let key = snapreq_collection::sealed::SecretKey::generate().unwrap();
    let imp = import(&fixture("commands.curl"), &Options::default()).unwrap();
    let dir = target("sealed");
    imp.write(&dir, Some(&key)).unwrap();
    let stored = shared_secrets(&dir);
    assert!(stored.contains("enc:v1:") && !stored.contains("curl-pass"));
}

#[test]
fn broken_inputs_are_errors_not_panics() {
    for (format, text) in [
        (Format::Postman, "{"),
        (Format::Postman, "{\"info\": {}}"),
        (Format::OpenApi, "openapi: 2.0"),
        (Format::OpenApi, "openapi: '3.0.0'\npaths: 7"),
        (Format::Insomnia, "{\"_type\": \"export\"}"),
        (Format::Har, "{\"log\": {}}"),
        (Format::Hurl, "GET https://a.test\n```\nunterminated"),
        (Format::Hurl, "nonsense"),
        (Format::Curl, "curl 'unterminated"),
    ] {
        let result = import(
            text,
            &Options {
                format: Some(format),
                name: None,
            },
        );
        if let Ok(imp) = result {
            // A document without operations imports as an empty collection, with a warning.
            assert!(!imp.warnings.is_empty(), "{format}: {text}");
        }
    }
}
