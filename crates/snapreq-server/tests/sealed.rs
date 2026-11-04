//! Secret values are sealed at rest (PLAN P8.4): plain values found at start are sealed, and
//! sealed values are used when sending.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::config::AuthConfig;
use snapreq_server::store::Retention;
use snapreq_server::{Config, Server};

#[tokio::test]
async fn plain_values_are_sealed_at_start_and_still_used() {
    let root = std::env::temp_dir().join(format!("snapreq-sealed-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let app = root.join("workspace/app");
    std::fs::create_dir_all(app.join(".secrets")).unwrap();
    std::fs::write(
        app.join("snapreq.toml"),
        "format = 1\nname = \"App\"\n[vars]\ntoken = { secret = true }\n",
    )
    .unwrap();
    std::fs::write(
        app.join(".secrets/shared.toml"),
        "token = \"plain-s3cret\"\n",
    )
    .unwrap();
    // A workspace subdirectory is covered too.
    let team = root.join("workspace/team/api");
    std::fs::create_dir_all(team.join(".secrets")).unwrap();
    std::fs::write(team.join("snapreq.toml"), "format = 1\nname = \"Api\"\n").unwrap();
    std::fs::write(team.join(".secrets/shared.toml"), "key = \"team-s3cret\"\n").unwrap();

    let config = Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: root.join("data"),
        workspace: root.join("workspace"),
        allow_remote: false,
        retention: Retention::default(),
        outbound: Default::default(),
        auth: AuthConfig::default(),
        oidc: None,
    };
    let server = Server::bind(config).await.unwrap();
    let addr = server.local_addr().unwrap();
    tokio::spawn(server.run(std::future::pending()));

    for file in [
        app.join(".secrets/shared.toml"),
        team.join(".secrets/shared.toml"),
    ] {
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(
            text.contains("enc:v1:") && !text.contains("s3cret"),
            "{text}"
        );
    }
    let key_file = root.join("data/secret.key");
    assert!(key_file.is_file());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key_file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    // The sealed value is sent: the server echoes nothing, so check through a request to itself
    // whose URL carries the secret and whose history hides it.
    std::fs::write(
        app.join("echo.toml"),
        format!("method = \"GET\"\nurl = \"http://{addr}/healthz?t={{{{token}}}}\"\n"),
    )
    .unwrap();
    let mut req = Request::new(
        http::Method::POST,
        format!("http://{addr}/api/v1/requests/execute"),
    );
    req.body = Body::Json {
        content: json!({ "collection": "app", "key": "echo" }).to_string(),
    };
    let resp = Client::default().execute(&req).await.unwrap();
    let doc: Value = serde_json::from_slice(&resp.body).unwrap();
    assert_eq!(doc["response"]["status"], 200, "{doc}");
    let trace = doc.to_string();
    assert!(!trace.contains("plain-s3cret"), "still hidden: {trace}");
    assert!(
        !trace.contains("enc:v1"),
        "the opened value was sent, not the sealed one"
    );
}
