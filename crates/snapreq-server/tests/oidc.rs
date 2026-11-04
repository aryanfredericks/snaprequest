//! Single sign-on end to end (PLAN P8.3) against a small OpenID Connect provider.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::collections::HashMap;
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use snapreq_core::{Client, Request, Response};
use snapreq_server::config::{AuthConfig, AuthMode, OidcConfig};
use snapreq_server::store::{Retention, Store};
use snapreq_server::{Config, Server};
use tokio::net::TcpListener;

const SECRET: &str = "client-secret-1";

#[derive(Default)]
struct Idp {
    /// Claims of the next person to log in.
    next: Value,
    /// code → (nonce, PKCE challenge, claims)
    codes: HashMap<String, (String, String, Value)>,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

async fn provider() -> (SocketAddr, Arc<Mutex<Idp>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let idp = Arc::new(Mutex::new(Idp::default()));
    let shared = idp.clone();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            let idp = shared.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                    let idp = idp.clone();
                    async move {
                        let issuer = format!("http://{addr}");
                        let path = req.uri().path().to_owned();
                        let query: HashMap<String, String> =
                            url::form_urlencoded::parse(req.uri().query().unwrap_or("").as_bytes())
                                .into_owned()
                                .collect();
                        let resp = match path.as_str() {
                            "/.well-known/openid-configuration" => {
                                hyper::Response::new(Full::new(Bytes::from(
                                    json!({
                                        "issuer": issuer,
                                        "authorization_endpoint": format!("{issuer}/authorize"),
                                        "token_endpoint": format!("{issuer}/token"),
                                    })
                                    .to_string(),
                                )))
                            }
                            "/authorize" => {
                                assert_eq!(query["client_id"], "snapreq");
                                assert_eq!(query["code_challenge_method"], "S256");
                                assert!(query["scope"].split(' ').any(|s| s == "openid"));
                                let code = format!(
                                    "code-{}",
                                    query["state"].len() + idp.lock().unwrap().codes.len()
                                );
                                let mut claims = idp.lock().unwrap().next.clone();
                                claims["iss"] = json!(issuer);
                                claims["aud"] = json!("snapreq");
                                claims["exp"] = json!(4_000_000_000_i64);
                                claims["nonce"] = json!(query["nonce"]);
                                idp.lock().unwrap().codes.insert(
                                    code.clone(),
                                    (
                                        query["nonce"].clone(),
                                        query["code_challenge"].clone(),
                                        claims,
                                    ),
                                );
                                hyper::Response::builder()
                                    .status(302)
                                    .header(
                                        "location",
                                        format!(
                                            "{}?code={code}&state={}",
                                            query["redirect_uri"], query["state"]
                                        ),
                                    )
                                    .body(Full::new(Bytes::new()))
                                    .unwrap()
                            }
                            "/token" => {
                                let body = req.into_body().collect().await.unwrap().to_bytes();
                                let form: HashMap<String, String> =
                                    url::form_urlencoded::parse(&body).into_owned().collect();
                                let found = idp.lock().unwrap().codes.remove(&form["code"]);
                                let (_, challenge, claims) = found.unwrap();
                                let digest = ring::digest::digest(
                                    &ring::digest::SHA256,
                                    form["code_verifier"].as_bytes(),
                                );
                                let ok = form["client_secret"] == SECRET
                                    && b64(digest.as_ref()) == challenge;
                                if ok {
                                    let jwt = format!(
                                        "{}.{}.sig",
                                        b64(br#"{"alg":"RS256"}"#),
                                        b64(claims.to_string().as_bytes())
                                    );
                                    hyper::Response::new(Full::new(Bytes::from(
                                        json!({ "id_token": jwt, "access_token": "a", "token_type": "Bearer" }).to_string(),
                                    )))
                                } else {
                                    hyper::Response::builder()
                                        .status(400)
                                        .body(Full::new(Bytes::from(
                                            r#"{"error":"invalid_grant"}"#,
                                        )))
                                        .unwrap()
                                }
                            }
                            _ => hyper::Response::builder()
                                .status(404)
                                .body(Full::new(Bytes::new()))
                                .unwrap(),
                        };
                        Ok::<_, Infallible>(resp)
                    }
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    (addr, idp)
}

struct Srv {
    base: String,
    store: Store,
    idp: Arc<Mutex<Idp>>,
}

async fn start(tag: &str, allowed_domains: &[&str]) -> Srv {
    let (idp_addr, idp) = provider().await;
    // The callback URL must be known before the server binds: reserve a port.
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let root = std::env::temp_dir().join(format!("snapreq-oidc-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let base = format!("http://127.0.0.1:{port}");
    let config = Config {
        bind: format!("127.0.0.1:{port}").parse().unwrap(),
        data_dir: root.join("data"),
        workspace: root.join("workspace"),
        allow_remote: false,
        retention: Retention::default(),
        outbound: Default::default(),
        auth: AuthConfig {
            mode: AuthMode::Accounts,
            ..AuthConfig::default()
        },
        oidc: Some(OidcConfig {
            issuer: format!("http://{idp_addr}"),
            client_id: "snapreq".into(),
            client_secret: Some(SECRET.into()),
            redirect_url: format!("{base}/api/v1/auth/oidc/callback"),
            name: "Test IdP".into(),
            scopes: vec!["openid".into(), "email".into()],
            allowed_domains: allowed_domains.iter().map(|d| (*d).to_owned()).collect(),
        }),
    };
    let server = Server::bind(config).await.unwrap();
    let store = server.store().clone();
    tokio::spawn(server.run(std::future::pending()));
    Srv { base, store, idp }
}

/// A browser: keeps cookies, follows redirects by hand.
struct Browser {
    client: Client,
}

impl Browser {
    fn new() -> Self {
        Self {
            client: Client::default(),
        }
    }

    async fn get(&self, url: &str) -> Response {
        let mut req = Request::get(url);
        req.settings.follow_redirects = false;
        self.client.execute(&req).await.unwrap()
    }

    /// Clicks "Log in with …" as `claims`; returns where the server finally sends the browser.
    async fn sso(&self, srv: &Srv, claims: Value) -> String {
        srv.idp.lock().unwrap().next = claims;
        let start = self
            .get(&format!("{}/api/v1/auth/oidc/start", srv.base))
            .await;
        assert_eq!(start.status, 302);
        let at_idp = self.get(start.header("location").unwrap()).await;
        assert_eq!(at_idp.status, 302, "{:?}", at_idp.text());
        let back = self.get(at_idp.header("location").unwrap()).await;
        assert_eq!(back.status, 302);
        back.header("location").unwrap().to_owned()
    }

    async fn status(&self, srv: &Srv) -> Value {
        let resp = self.get(&format!("{}/api/v1/auth/status", srv.base)).await;
        serde_json::from_slice(&resp.body).unwrap()
    }
}

fn error_of(location: &str) -> String {
    let q = location.split_once('?').map(|(_, q)| q).unwrap_or_default();
    url::form_urlencoded::parse(q.as_bytes())
        .find(|(k, _)| k == "login_error")
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

#[tokio::test]
async fn the_first_login_creates_an_account_and_later_ones_reuse_it() {
    let srv = start("create", &[]).await;
    let browser = Browser::new();
    assert_eq!(
        browser.status(&srv).await["oidc"],
        json!({ "name": "Test IdP" })
    );
    let to = browser
        .sso(
            &srv,
            json!({ "sub": "u-1", "preferred_username": "ana", "email": "ana@example.com" }),
        )
        .await;
    assert_eq!(to, "/");
    let status = browser.status(&srv).await;
    assert_eq!(status["user"]["username"], "ana");
    let ana = status["user"]["id"].as_i64().unwrap();
    // The first account owns the default workspace.
    assert_eq!(
        srv.store.workspaces(Some(ana)).await.unwrap()[0].id,
        "default"
    );
    // It has no password.
    assert!(srv.store.check_login("ana", "!").await.unwrap().is_none());

    // The same identity again, from another browser: the same account.
    let other = Browser::new();
    other
        .sso(
            &srv,
            json!({ "sub": "u-1", "preferred_username": "renamed" }),
        )
        .await;
    assert_eq!(other.status(&srv).await["user"]["id"], ana);
    assert_eq!(srv.store.users().await.unwrap().len(), 1);
}

#[tokio::test]
async fn an_identity_never_takes_over_a_local_account() {
    let srv = start("takeover", &[]).await;
    srv.store.create_user("bo", "bo-password").await.unwrap();
    let browser = Browser::new();
    browser
        .sso(&srv, json!({ "sub": "u-2", "preferred_username": "bo" }))
        .await;
    let status = browser.status(&srv).await;
    assert_eq!(status["user"]["username"], "bo-2");
    // The second account joins no workspace.
    let id = status["user"]["id"].as_i64().unwrap();
    assert!(srv.store.workspaces(Some(id)).await.unwrap().is_empty());
}

#[tokio::test]
async fn allowed_domains_need_a_verified_address() {
    let srv = start("domains", &["example.com"]).await;
    let browser = Browser::new();
    for claims in [
        json!({ "sub": "a", "email": "ana@evil.com", "email_verified": true }),
        json!({ "sub": "b", "email": "ana@example.com", "email_verified": false }),
        json!({ "sub": "c" }),
    ] {
        let to = browser.sso(&srv, claims).await;
        assert!(error_of(&to).contains("allowed domains"), "{to}");
        assert_eq!(browser.status(&srv).await["user"], Value::Null);
    }
    browser
        .sso(
            &srv,
            json!({ "sub": "d", "email": "cy@Example.com", "email_verified": true }),
        )
        .await;
    assert_eq!(browser.status(&srv).await["user"]["username"], "cy");
}

#[tokio::test]
async fn a_callback_from_another_browser_is_refused() {
    let srv = start("csrf", &[]).await;
    let victim = Browser::new();
    let attacker = Browser::new();
    srv.idp.lock().unwrap().next = json!({ "sub": "attacker" });
    let start = attacker
        .get(&format!("{}/api/v1/auth/oidc/start", srv.base))
        .await;
    let at_idp = attacker.get(start.header("location").unwrap()).await;
    // The attacker makes the victim's browser open the callback with the attacker's code.
    let back = victim.get(at_idp.header("location").unwrap()).await;
    assert!(error_of(back.header("location").unwrap()).contains("another browser"));
    assert_eq!(victim.status(&srv).await["user"], Value::Null);
    // A made-up state with a matching cookie is refused too.
    let fake = Browser::new();
    fake.get(&format!("{}/api/v1/auth/oidc/start", srv.base))
        .await;
    let back = fake
        .get(&format!(
            "{}/api/v1/auth/oidc/callback?code=x&state=forged",
            srv.base
        ))
        .await;
    assert!(!error_of(back.header("location").unwrap()).is_empty());
    // And provider errors come back as messages.
    let back = fake
        .get(&format!(
            "{}/api/v1/auth/oidc/callback?error=access_denied&error_description=nope",
            srv.base
        ))
        .await;
    assert!(error_of(back.header("location").unwrap()).contains("nope"));
}
