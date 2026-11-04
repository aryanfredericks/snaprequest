//! Backup and restore (PLAN P8.6): a backup taken while the server runs restores into a working
//! server with the same accounts, workspaces, history and secrets.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use snapreq_core::{Body, Client, Request};
use snapreq_server::config::{AuthConfig, AuthMode};
use snapreq_server::store::{HistoryQuery, Retention, Store};
use snapreq_server::{Config, Server, backup};

fn config(root: &Path) -> Config {
    Config {
        bind: "127.0.0.1:0".parse().unwrap(),
        data_dir: root.join("data"),
        workspace: root.join("workspace"),
        allow_remote: false,
        retention: Retention::default(),
        outbound: Default::default(),
        auth: AuthConfig {
            mode: AuthMode::Accounts,
            ..AuthConfig::default()
        },
        oidc: None,
    }
}

fn fresh(tag: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("snapreq-backup-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    root
}

async fn call(
    base: &str,
    token: &str,
    method: &str,
    path: &str,
    ws: &str,
    body: Option<Value>,
) -> (u16, Value) {
    let mut req = Request::new(
        http::Method::from_bytes(method.as_bytes()).unwrap(),
        format!("{base}/api/v1{path}"),
    )
    .header("Authorization", format!("Bearer {token}"))
    .header("X-SnapReq-Workspace", ws);
    if let Some(b) = body {
        req.body = Body::Json {
            content: b.to_string(),
        };
    }
    let resp = Client::default().execute(&req).await.unwrap();
    (
        resp.status,
        serde_json::from_slice(&resp.body).unwrap_or(Value::Null),
    )
}

/// A running server with an account, a workspace, a collection with a secret and some history.
async fn populated(root: &Path) -> (Server, String, String) {
    let server = Server::bind(config(root)).await.unwrap();
    let base = format!("http://{}", server.local_addr().unwrap());
    let store = server.store();
    let ana = store.create_user("ana", "ana-password").await.unwrap();
    let (_, token) = store.create_token(ana.id, "t", None).await.unwrap();
    (server, base, token)
}

async fn fill(base: &str, token: &str) {
    assert_eq!(
        call(
            base,
            token,
            "POST",
            "/workspaces",
            "default",
            Some(json!({ "id": "team" }))
        )
        .await
        .0,
        201
    );
    assert_eq!(
        call(
            base,
            token,
            "POST",
            "/collections",
            "team",
            Some(json!({ "id": "shop" }))
        )
        .await
        .0,
        201
    );
    let (status, _) = call(
        base,
        token,
        "PUT",
        "/collections/shop",
        "team",
        Some(json!({ "format": 1, "name": "Shop", "vars": { "token": { "secret": true } } })),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(
        call(
            base,
            token,
            "PUT",
            "/collections/shop/secrets/shared/token",
            "team",
            Some(json!({ "value": "s3cr3t" }))
        )
        .await
        .0,
        204
    );
    let (status, _) = call(
        base,
        token,
        "POST",
        "/requests/execute",
        "default",
        Some(json!({ "request": { "method": "GET", "url": format!("{base}/healthz") } })),
    )
    .await;
    assert_eq!(status, 200);
}

async fn secret_is_set(base: &str, token: &str) -> bool {
    let (_, doc) = call(
        base,
        token,
        "GET",
        "/collections/shop/variables",
        "team",
        None,
    )
    .await;
    doc["variables"]
        .as_array()
        .unwrap()
        .iter()
        .any(|v| v["name"] == "token" && v["has_value"] == true)
}

#[tokio::test]
async fn a_backup_restores_into_a_working_server() {
    let old = fresh("old");
    let (server, base, token) = populated(&old).await;
    let store: Store = server.store().clone();
    tokio::spawn(server.run(std::future::pending()));
    fill(&base, &token).await;

    // Taken while the server runs.
    let mut archive = Vec::new();
    let m = backup::create(&config(&old), &store, &mut archive, true)
        .await
        .unwrap();
    assert!(
        m.includes_key && m.schema >= 6 && m.workspace_files >= 3,
        "{m:?}"
    );
    assert!(!String::from_utf8_lossy(&archive).contains("s3cr3t"));

    let new = fresh("new");
    let restored = backup::restore(&config(&new), archive.as_slice(), false).unwrap();
    assert_eq!(restored, m);
    assert!(new.join("workspace/team/shop/snapreq.toml").is_file());
    let server = Server::bind(config(&new)).await.unwrap();
    let base = format!("http://{}", server.local_addr().unwrap());
    let store = server.store().clone();
    tokio::spawn(server.run(std::future::pending()));
    // Same token, workspaces, history and secrets.
    let (status, doc) = call(&base, &token, "GET", "/workspaces", "default", None).await;
    assert_eq!(status, 200);
    assert_eq!(doc["workspaces"].as_array().unwrap().len(), 2);
    assert!(secret_is_set(&base, &token).await);
    let history = store
        .history(
            Some(1),
            HistoryQuery {
                limit: 10,
                ..HistoryQuery::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(history.len(), 1);
    assert!(
        store
            .check_login("ana", "ana-password")
            .await
            .unwrap()
            .is_some()
    );

    // Restoring over data needs --force.
    let err = backup::restore(&config(&new), archive.as_slice(), false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("--force"), "{err}");
}

#[tokio::test]
async fn without_the_key_sealed_secrets_stay_closed() {
    let old = fresh("nokey-old");
    let (server, base, token) = populated(&old).await;
    let store = server.store().clone();
    tokio::spawn(server.run(std::future::pending()));
    fill(&base, &token).await;
    let mut archive = Vec::new();
    let m = backup::create(&config(&old), &store, &mut archive, false)
        .await
        .unwrap();
    assert!(!m.includes_key);

    let new = fresh("nokey-new");
    backup::restore(&config(&new), archive.as_slice(), false).unwrap();
    assert!(!new.join("data/secret.key").exists());
    // With the old key put back, they open.
    std::fs::copy(old.join("data/secret.key"), new.join("data/secret.key")).unwrap();
    let server = Server::bind(config(&new)).await.unwrap();
    let base = format!("http://{}", server.local_addr().unwrap());
    tokio::spawn(server.run(std::future::pending()));
    assert!(secret_is_set(&base, &token).await);
}

#[tokio::test]
async fn broken_or_malicious_archives_change_nothing() {
    let new = fresh("evil");
    // Not an archive.
    let err = backup::restore(&config(&new), b"hello".as_slice(), false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a SnapRequest backup"), "{err}");
    // A path leaving the archive.
    let mut raw = Vec::new();
    {
        let gz = flate2::write::GzEncoder::new(&mut raw, flate2::Compression::default());
        let mut tar = tar::Builder::new(gz);
        let mut h = tar::Header::new_gnu();
        let data = b"pwned";
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        // Bypass the builder's own path check, as a hand-made archive would.
        h.as_gnu_mut().unwrap().name[..12].copy_from_slice(b"../pwned.txt");
        h.set_cksum();
        tar.append(&h, data.as_slice()).unwrap();
        tar.into_inner().unwrap().finish().unwrap();
    }
    let err = backup::restore(&config(&new), raw.as_slice(), false)
        .unwrap_err()
        .to_string();
    assert!(err.contains("not a SnapRequest backup"), "{err}");
    assert!(!new.join("pwned.txt").exists() && !new.join("data/../pwned.txt").exists());
    assert!(!new.join("data/snapreq.db").exists());
}
