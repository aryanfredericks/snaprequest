//! OAuth 2.0 against a local mock authorization server.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use snapreq_core::auth::{AuthorizationPrompt, MemoryTokenStore, TokenStore};
use snapreq_core::model::{Auth, OAuth2, OAuth2Grant, Phase, TraceEntry, TraceEvent};
use snapreq_core::{Client, ClientConfig, Error, Request};
use tokio::net::TcpListener;

#[derive(Default)]
struct State {
    /// `grant_type` of every token request, in order.
    log: Vec<String>,
    tokens: HashSet<String>,
    refresh_tokens: HashSet<String>,
    /// code → (code_challenge, redirect_uri)
    codes: HashMap<String, (String, String)>,
    next: u32,
}

type Shared = Arc<Mutex<State>>;

fn json(status: u16, body: String) -> hyper::Response<Full<Bytes>> {
    hyper::Response::builder()
        .status(status)
        .header("content-type", "application/json")
        .body(Full::new(Bytes::from(body)))
        .unwrap()
}

fn issue(state: &mut State, client: &str, refresh: bool) -> hyper::Response<Full<Bytes>> {
    state.next += 1;
    let token = format!("access-{}-{}", client, state.next);
    state.tokens.insert(token.clone());
    // `short` tokens are always within the renewal margin.
    let expires = if client == "short" { 5 } else { 3600 };
    let refresh = if refresh {
        let r = format!("refresh-{}", state.next);
        state.refresh_tokens.insert(r.clone());
        format!(r#","refresh_token":"{r}""#)
    } else {
        String::new()
    };
    json(
        200,
        format!(
            r#"{{"access_token":"{token}","token_type":"Bearer","expires_in":{expires}{refresh}}}"#
        ),
    )
}

fn s256(verifier: &str) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash.as_ref())
}

/// Routes:
/// - `POST /token`: grants `client_credentials`, `password` (alice / wonderland),
///   `refresh_token` (no new refresh token is returned) and `authorization_code` (PKCE S256).
///   Confidential clients authenticate with Basic `app:secret` or `short:secret`; `spa` is public.
/// - `GET /authorize`: redirects straight back with a code (the user "approves").
/// - `GET /api`: 200 for a valid token, else 401.
/// - `POST /revoke`: invalidates every access token.
async fn handle(
    state: Shared,
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let path = req.uri().path().to_owned();
    let query: HashMap<String, String> =
        url::form_urlencoded::parse(req.uri().query().unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    let authorization = req
        .headers()
        .get("authorization")
        .map(|v| v.to_str().unwrap().to_owned())
        .unwrap_or_default();
    let body = req.into_body().collect().await.unwrap().to_bytes();
    let form: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
    let mut st = state.lock().unwrap();

    Ok(match path.as_str() {
        "/token" => {
            let grant = form.get("grant_type").cloned().unwrap_or_default();
            st.log.push(grant.clone());
            let client = match authorization.strip_prefix("Basic ") {
                Some(b64) => {
                    let raw = base64::engine::general_purpose::STANDARD
                        .decode(b64)
                        .unwrap();
                    match String::from_utf8(raw).unwrap().as_str() {
                        "app:secret" => "app".to_owned(),
                        "short:secret" => "short".to_owned(),
                        _ => return Ok(json(401, r#"{"error":"invalid_client"}"#.into())),
                    }
                }
                None if form.get("client_id").map(String::as_str) == Some("spa") => "spa".into(),
                None => return Ok(json(401, r#"{"error":"invalid_client"}"#.into())),
            };
            match grant.as_str() {
                "client_credentials" => issue(&mut st, &client, false),
                "password" => {
                    if form.get("username").map(String::as_str) == Some("alice")
                        && form.get("password").map(String::as_str) == Some("wonderland")
                    {
                        issue(&mut st, &client, true)
                    } else {
                        json(
                            400,
                            r#"{"error":"invalid_grant","error_description":"wrong password"}"#
                                .into(),
                        )
                    }
                }
                "refresh_token" => {
                    let r = form.get("refresh_token").cloned().unwrap_or_default();
                    if st.refresh_tokens.contains(&r) {
                        issue(&mut st, &client, false)
                    } else {
                        json(400, r#"{"error":"invalid_grant"}"#.into())
                    }
                }
                "authorization_code" => {
                    let code = form.get("code").cloned().unwrap_or_default();
                    let ok = st.codes.remove(&code).is_some_and(|(challenge, redirect)| {
                        form.get("redirect_uri") == Some(&redirect)
                            && form.get("code_verifier").map(|v| s256(v)) == Some(challenge)
                    });
                    if ok {
                        issue(&mut st, &client, true)
                    } else {
                        json(400, r#"{"error":"invalid_grant"}"#.into())
                    }
                }
                _ => json(400, r#"{"error":"unsupported_grant_type"}"#.into()),
            }
        }
        "/authorize" => {
            assert_eq!(query["response_type"], "code");
            assert_eq!(query["code_challenge_method"], "S256");
            st.next += 1;
            let code = format!("code-{}", st.next);
            let redirect = query["redirect_uri"].clone();
            st.codes.insert(
                code.clone(),
                (query["code_challenge"].clone(), redirect.clone()),
            );
            hyper::Response::builder()
                .status(302)
                .header(
                    "location",
                    format!("{redirect}?code={code}&state={}", query["state"]),
                )
                .body(Full::new(Bytes::new()))
                .unwrap()
        }
        "/api" => match authorization.strip_prefix("Bearer ") {
            Some(t) if st.tokens.contains(t) => json(200, r#"{"hello":"world"}"#.into()),
            _ => json(401, String::new()),
        },
        "/revoke" => {
            st.tokens.clear();
            json(200, String::new())
        }
        _ => json(404, String::new()),
    })
}

async fn start() -> (SocketAddr, Shared) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let state = Shared::default();
    let shared = state.clone();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let state = shared.clone();
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(
                        TokioIo::new(stream),
                        service_fn(move |r| handle(state.clone(), r)),
                    )
                    .await;
            });
        }
    });
    (addr, state)
}

fn config(addr: SocketAddr, grant: OAuth2Grant, client: &str) -> OAuth2 {
    let mut o = OAuth2::new(grant, format!("http://{addr}/token"), client);
    if client != "spa" {
        o.client_secret = "secret".into();
    }
    o
}

fn api(addr: SocketAddr, o: &OAuth2) -> Request {
    Request::get(format!("http://{addr}/api")).auth(Auth::OAuth2(o.clone()))
}

fn log(state: &Shared) -> Vec<String> {
    state.lock().unwrap().log.clone()
}

fn infos(trace: &[TraceEntry]) -> Vec<String> {
    trace
        .iter()
        .filter_map(|e| match &e.event {
            TraceEvent::Info { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn execute_futures_stay_send() {
    fn is_send<T: Send>(_: &T) {}
    let client = Client::default();
    let req = Request::get("http://x.test");
    is_send(&client.execute(&req));
}

#[tokio::test]
async fn client_credentials_token_is_fetched_once_and_cached() {
    let (addr, state) = start().await;
    let client = Client::default();
    let o = config(addr, OAuth2Grant::ClientCredentials, "app");

    let first = client.execute(&api(addr, &o)).await.unwrap();
    assert_eq!(first.status, 200);
    assert!(
        infos(&first.trace)[0].starts_with(&format!(
            "oauth2: got an access token from http://{addr}/token (client_credentials grant, expires in 3"
        )),
        "{:?}",
        infos(&first.trace)
    );
    let trace = serde_json::to_string(&first.trace).unwrap();
    assert!(!trace.contains("access-app"), "{trace}");
    assert!(trace.contains("Bearer <redacted>"), "{trace}");

    let second = client.execute(&api(addr, &o)).await.unwrap();
    assert_eq!(second.status, 200);
    assert!(infos(&second.trace)[0].starts_with("oauth2: using the cached access token"));
    assert_eq!(log(&state), ["client_credentials"]);

    // Another scope is another token.
    let mut other = o.clone();
    other.scope = "admin".into();
    client.execute(&api(addr, &other)).await.unwrap();
    assert_eq!(log(&state), ["client_credentials", "client_credentials"]);
}

#[tokio::test]
async fn expiring_tokens_are_refreshed_and_the_refresh_token_is_kept() {
    let (addr, state) = start().await;
    let client = Client::default();
    let mut o = config(addr, OAuth2Grant::Password, "short");
    o.username = "alice".into();
    o.password = "wonderland".into();

    for _ in 0..3 {
        assert_eq!(client.execute(&api(addr, &o)).await.unwrap().status, 200);
    }
    // The server does not send a new refresh token, so the first one is reused.
    assert_eq!(log(&state), ["password", "refresh_token", "refresh_token"]);
}

#[tokio::test]
async fn a_failed_refresh_falls_back_to_a_new_grant() {
    let (addr, state) = start().await;
    let store = Arc::new(MemoryTokenStore::default());
    let client = Client::new(ClientConfig {
        token_store: store.clone(),
        ..ClientConfig::default()
    })
    .unwrap();
    let mut o = config(addr, OAuth2Grant::Password, "short");
    o.username = "alice".into();
    o.password = "wonderland".into();
    client.execute(&api(addr, &o)).await.unwrap();
    state.lock().unwrap().refresh_tokens.clear();

    let resp = client.execute(&api(addr, &o)).await.unwrap();
    assert_eq!(resp.status, 200);
    assert_eq!(log(&state), ["password", "refresh_token", "password"]);
    assert!(
        infos(&resp.trace)[0]
            .contains("refreshing failed (the token endpoint answered 400: invalid_grant)"),
        "{:?}",
        infos(&resp.trace)
    );
}

#[tokio::test]
async fn a_rejected_cached_token_is_replaced_once() {
    let (addr, state) = start().await;
    let client = Client::default();
    let o = config(addr, OAuth2Grant::ClientCredentials, "app");
    client.execute(&api(addr, &o)).await.unwrap();
    client
        .execute(&Request::new(
            http::Method::POST,
            format!("http://{addr}/revoke"),
        ))
        .await
        .unwrap();

    let resp = client.execute(&api(addr, &o)).await.unwrap();
    assert_eq!(resp.status, 200);
    assert!(resp.trace.iter().any(|e| matches!(
        &e.event,
        TraceEvent::AuthChallenge { status: 401, scheme } if scheme == "oauth2"
    )));
    assert_eq!(log(&state), ["client_credentials", "client_credentials"]);
}

#[tokio::test]
async fn token_errors_fail_in_the_auth_phase_without_secrets() {
    let (addr, _) = start().await;
    let client = Client::default();
    let mut o = config(addr, OAuth2Grant::Password, "app");
    o.username = "alice".into();
    o.password = "not-the-password".into();
    let Err(Error::Execution(e)) = client.execute(&api(addr, &o)).await else {
        panic!("expected an execution error");
    };
    assert_eq!(e.phase, Phase::Auth);
    assert_eq!(
        e.message,
        "the token endpoint answered 400: invalid_grant (wrong password)"
    );
    assert!(
        !serde_json::to_string(&e.trace)
            .unwrap()
            .contains("not-the-password")
    );

    let mut bad_client = config(addr, OAuth2Grant::ClientCredentials, "app");
    bad_client.client_secret = "wrong-secret".into();
    let Err(Error::Execution(e)) = client.execute(&api(addr, &bad_client)).await else {
        panic!("expected an execution error");
    };
    assert_eq!(e.message, "the token endpoint answered 401: invalid_client");

    let down = OAuth2::new(
        OAuth2Grant::ClientCredentials,
        "http://127.0.0.1:1/token",
        "x",
    );
    let Err(Error::Execution(e)) = client.execute(&api(addr, &down)).await else {
        panic!("expected an execution error");
    };
    assert_eq!(e.phase, Phase::Auth);
    assert!(
        e.message
            .starts_with("token request to http://127.0.0.1:1/token failed"),
        "{}",
        e.message
    );
}

/// Plays the user: loads the authorization URL like a browser, which redirects to the
/// local listener.
#[derive(Debug, Default)]
struct Browser {
    opened: Mutex<Vec<String>>,
}

impl AuthorizationPrompt for Browser {
    fn open(&self, url: &str) -> Result<(), String> {
        self.opened.lock().unwrap().push(url.to_owned());
        let url = url.to_owned();
        tokio::spawn(async move {
            let _ = Client::default().execute(&Request::get(url)).await;
        });
        Ok(())
    }
}

#[tokio::test]
async fn authorization_code_with_pkce_through_the_browser() {
    let (addr, state) = start().await;
    let browser = Arc::new(Browser::default());
    let client = Client::new(ClientConfig {
        authorization_prompt: Some(browser.clone()),
        ..ClientConfig::default()
    })
    .unwrap();
    let mut o = config(addr, OAuth2Grant::AuthorizationCode, "spa");
    o.auth_url = Some(format!("http://{addr}/authorize"));
    o.scope = "openid".into();

    let resp = client.execute(&api(addr, &o)).await.unwrap();
    assert_eq!(resp.status, 200, "{:?}", infos(&resp.trace));
    assert!(infos(&resp.trace)[0].starts_with("oauth2: waiting for authorization in the browser"));
    let opened = browser.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1);
    assert!(opened[0].contains("client_id=spa") && opened[0].contains("scope=openid"));

    client.execute(&api(addr, &o)).await.unwrap();
    assert_eq!(
        browser.opened.lock().unwrap().len(),
        1,
        "the token is cached"
    );
    assert_eq!(log(&state), ["authorization_code"]);

    // Without a prompt the grant cannot work.
    let Err(Error::Execution(e)) = Client::default().execute(&api(addr, &o)).await else {
        panic!("expected an execution error");
    };
    assert!(e.message.contains("needs a browser"), "{}", e.message);
}

#[tokio::test]
async fn clients_sharing_a_store_share_tokens() {
    let (addr, state) = start().await;
    let store: Arc<dyn TokenStore> = Arc::new(MemoryTokenStore::default());
    let make = || {
        Client::new(ClientConfig {
            token_store: store.clone(),
            ..ClientConfig::default()
        })
        .unwrap()
    };
    let o = config(addr, OAuth2Grant::ClientCredentials, "app");
    make().execute(&api(addr, &o)).await.unwrap();
    make().execute(&api(addr, &o)).await.unwrap();
    assert_eq!(log(&state), ["client_credentials"]);
}
