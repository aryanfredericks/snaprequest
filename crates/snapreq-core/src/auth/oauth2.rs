//! OAuth 2.0 (RFC 6749): token requests, PKCE (RFC 7636), the loopback redirect listener
//! (RFC 8252) and the token cache interface.
//!
//! The engine drives the flow (see `Client::oauth_token`); this module holds the pieces
//! that do not need a client.

use std::collections::HashMap;
use std::fmt::Debug;
use std::sync::Mutex;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;
use url::Url;

use crate::model::{Auth, Body, ClientAuth, KeyValue, OAuth2, OAuth2Grant, Request, Response};

/// A token is renewed this long before it expires, so it does not expire in flight.
const EXPIRY_MARGIN: Duration = Duration::from_secs(10);

/// How long to wait for the browser to come back to the redirect URI.
pub(crate) const AUTHORIZATION_TIMEOUT: Duration = Duration::from_secs(300);

/// An access token as stored in a [`TokenStore`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub token_type: String,
    /// Expiry as Unix seconds; `None` if the server did not say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// Scopes granted, if the server said.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
}

impl Token {
    /// Whether the token can still be used for a while. Tokens without an expiry are
    /// used until the server rejects them.
    pub fn is_fresh(&self, now: SystemTime) -> bool {
        let Some(at) = self.expires_at else {
            return true;
        };
        unix(now) + EXPIRY_MARGIN.as_secs() < at
    }

    /// Seconds until expiry, if known.
    pub fn expires_in(&self, now: SystemTime) -> Option<u64> {
        self.expires_at.map(|at| at.saturating_sub(unix(now)))
    }
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs()
}

/// Where a [`Client`](crate::Client) keeps OAuth 2.0 tokens between requests.
///
/// The default keeps them in memory for the life of the client; front ends can persist
/// them (the CLI uses a file in the user's cache directory). Keys contain no secrets.
pub trait TokenStore: Debug + Send + Sync {
    fn get(&self, key: &str) -> Option<Token>;
    fn put(&self, key: &str, token: &Token);
    fn remove(&self, key: &str);
}

/// A [`TokenStore`] in memory.
#[derive(Debug, Default)]
pub struct MemoryTokenStore(Mutex<HashMap<String, Token>>);

impl TokenStore for MemoryTokenStore {
    fn get(&self, key: &str) -> Option<Token> {
        self.0.lock().ok()?.get(key).cloned()
    }

    fn put(&self, key: &str, token: &Token) {
        if let Ok(mut m) = self.0.lock() {
            m.insert(key.to_owned(), token.clone());
        }
    }

    fn remove(&self, key: &str) {
        if let Ok(mut m) = self.0.lock() {
            m.remove(key);
        }
    }
}

/// Shows the user the authorization URL of the authorization code grant, for example by
/// opening a browser. Front ends without a user cannot use that grant.
pub trait AuthorizationPrompt: Debug + Send + Sync {
    /// Called once per authorization; returning an error aborts it.
    fn open(&self, url: &str) -> Result<(), String>;
}

/// The cache key for a configuration: the same server, client, grant, scopes and user
/// share a token.
pub(crate) fn cache_key(o: &OAuth2) -> String {
    format!(
        "{} {} client={} scope={} user={}",
        o.grant.as_str(),
        o.token_url.trim(),
        o.client_id,
        o.scope,
        o.username
    )
}

/// A form `POST` to the token endpoint with the client's credentials.
/// `params` are the grant-specific parameters.
pub(crate) fn token_request(
    o: &OAuth2,
    params: Vec<(&str, String)>,
    with_extra: bool,
    base: &Request,
) -> Request {
    // Credentials in the form are hidden in the token request's own trace.
    let mut secrets = base.secrets();
    secrets.push(o.client_secret.clone());
    for (name, value) in &params {
        if matches!(
            *name,
            "password" | "refresh_token" | "code" | "code_verifier"
        ) {
            secrets.push(value.clone());
        }
    }
    let mut fields: Vec<KeyValue> = params
        .into_iter()
        .map(|(k, v)| KeyValue::new(k, v))
        .collect();
    let mut req =
        Request::new(http::Method::POST, o.token_url.trim()).header("Accept", "application/json");
    if o.client_secret.is_empty() {
        fields.push(KeyValue::new("client_id", &o.client_id));
    } else {
        match o.client_auth {
            // RFC 6749 section 2.3.1: id and secret are form-encoded before Basic encoding.
            ClientAuth::Basic => {
                req.auth = Auth::Basic {
                    username: form_encode(&o.client_id),
                    password: form_encode(&o.client_secret),
                };
            }
            ClientAuth::Body => {
                fields.push(KeyValue::new("client_id", &o.client_id));
                fields.push(KeyValue::new("client_secret", &o.client_secret));
            }
        }
    }
    if with_extra {
        fields.extend(o.params.iter().filter(|p| p.enabled).cloned());
    }
    req.body = Body::Form { fields };
    // Same network settings as the request that needs the token.
    req.settings = base.settings.clone();
    req.settings.capture_wire = false;
    secrets.retain(|s| !s.is_empty());
    secrets.dedup();
    req.settings.secrets = secrets;
    req
}

fn form_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// Grant parameters for the initial token request (not the authorization code grant).
pub(crate) fn grant_params(o: &OAuth2) -> Vec<(&'static str, String)> {
    let mut p = vec![("grant_type", o.grant.as_str().to_owned())];
    if o.grant == OAuth2Grant::Password {
        p.push(("username", o.username.clone()));
        p.push(("password", o.password.clone()));
    }
    if !o.scope.is_empty() {
        p.push(("scope", o.scope.clone()));
    }
    p
}

pub(crate) fn refresh_params(o: &OAuth2, refresh_token: &str) -> Vec<(&'static str, String)> {
    let mut p = vec![
        ("grant_type", "refresh_token".to_owned()),
        ("refresh_token", refresh_token.to_owned()),
    ];
    if !o.scope.is_empty() {
        p.push(("scope", o.scope.clone()));
    }
    p
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    token_type: Option<String>,
    #[serde(default)]
    expires_in: Option<serde_json::Value>,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    scope: Option<String>,
}

#[derive(Deserialize)]
struct ErrorResponse {
    error: String,
    #[serde(default)]
    error_description: Option<String>,
}

/// Reads a token endpoint response (RFC 6749 sections 5.1 and 5.2).
pub(crate) fn parse_token_response(resp: &Response, now: SystemTime) -> Result<Token, String> {
    let body = resp.body.as_ref();
    if !(200..300).contains(&resp.status) {
        return Err(match serde_json::from_slice::<ErrorResponse>(body) {
            Ok(e) => match e.error_description {
                Some(d) => format!(
                    "the token endpoint answered {}: {} ({d})",
                    resp.status, e.error
                ),
                None => format!("the token endpoint answered {}: {}", resp.status, e.error),
            },
            Err(_) => format!("the token endpoint answered {}", resp.status),
        });
    }
    let t: TokenResponse = serde_json::from_slice(body).map_err(|e| {
        format!(
            "the token endpoint answered {} without a usable token: {e}",
            resp.status
        )
    })?;
    if t.access_token.is_empty() {
        return Err("the token endpoint returned an empty access_token".into());
    }
    // Some servers send `expires_in` as a string.
    let expires_in = match t.expires_in {
        Some(serde_json::Value::Number(n)) => n.as_u64(),
        Some(serde_json::Value::String(s)) => s.trim().parse().ok(),
        _ => None,
    };
    Ok(Token {
        access_token: t.access_token,
        token_type: t.token_type.unwrap_or_else(|| "Bearer".into()),
        expires_at: expires_in.map(|s| unix(now) + s),
        refresh_token: t.refresh_token.filter(|r| !r.is_empty()),
        scope: t.scope,
    })
}

fn random_b64(len: usize) -> String {
    use ring::rand::SecureRandom as _;
    let mut bytes = vec![0u8; len];
    let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// A PKCE verifier and its S256 challenge (RFC 7636).
pub(crate) fn pkce() -> (String, String) {
    let verifier = random_b64(32);
    (verifier.clone(), pkce_challenge(&verifier))
}

fn pkce_challenge(verifier: &str) -> String {
    let hash = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hash.as_ref())
}

pub(crate) fn random_state() -> String {
    random_b64(16)
}

/// Binds the loopback listener for `redirect_uri` and returns it with the URI to send
/// (with the real port if the configured one is `0` or absent).
pub(crate) async fn redirect_listener(o: &OAuth2) -> Result<(TcpListener, Url), String> {
    let raw = o
        .redirect_uri
        .as_deref()
        .unwrap_or("http://127.0.0.1:0/callback");
    let mut uri = Url::parse(raw).map_err(|e| format!("invalid redirect_uri `{raw}`: {e}"))?;
    let ip: std::net::IpAddr = match uri.host() {
        Some(url::Host::Ipv4(ip)) if ip.is_loopback() => ip.into(),
        Some(url::Host::Ipv6(ip)) if ip.is_loopback() => ip.into(),
        Some(url::Host::Domain("localhost")) => std::net::Ipv4Addr::LOCALHOST.into(),
        _ => {
            return Err(format!(
                "redirect_uri `{raw}` must be a loopback address (127.0.0.1, [::1] or localhost) so SnapRequest can receive the code"
            ));
        }
    };
    if uri.scheme() != "http" {
        return Err(format!(
            "redirect_uri `{raw}` must use http (a local listener has no certificate)"
        ));
    }
    let port = uri.port().unwrap_or(80);
    let listener = TcpListener::bind((ip, port))
        .await
        .map_err(|e| format!("cannot listen on {ip}:{port} for the redirect: {e}"))?;
    if port == 0 {
        let actual = listener.local_addr().map_err(|e| e.to_string())?.port();
        let _ = uri.set_port(Some(actual));
    }
    Ok((listener, uri))
}

/// The authorization request URL (RFC 6749 section 4.1.1, with PKCE).
pub(crate) fn authorization_url(
    o: &OAuth2,
    redirect_uri: &Url,
    state: &str,
    challenge: &str,
) -> Result<Url, String> {
    let raw = o
        .auth_url
        .as_deref()
        .map(str::trim)
        .filter(|u| !u.is_empty())
        .ok_or("the authorization code grant needs `auth_url`")?;
    let mut url = Url::parse(raw).map_err(|e| format!("invalid auth_url `{raw}`: {e}"))?;
    {
        let mut q = url.query_pairs_mut();
        q.append_pair("response_type", "code");
        q.append_pair("client_id", &o.client_id);
        q.append_pair("redirect_uri", redirect_uri.as_str());
        if !o.scope.is_empty() {
            q.append_pair("scope", &o.scope);
        }
        q.append_pair("state", state);
        q.append_pair("code_challenge", challenge);
        q.append_pair("code_challenge_method", "S256");
        for p in o.params.iter().filter(|p| p.enabled) {
            q.append_pair(&p.name, &p.value);
        }
    }
    Ok(url)
}

/// Waits for the browser to arrive at the redirect URI and returns the authorization code.
/// Requests for other paths (a favicon, say) get a 404 and waiting continues.
pub(crate) async fn wait_for_code(
    listener: &TcpListener,
    redirect_uri: &Url,
    state: &str,
) -> Result<String, String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("redirect listener failed: {e}"))?;
        let Some(target) = read_request_target(&mut stream).await else {
            continue;
        };
        let Ok(url) = redirect_uri.join(&target) else {
            respond(&mut stream, "400 Bad Request", "Bad request.").await;
            continue;
        };
        if url.path() != redirect_uri.path() {
            respond(&mut stream, "404 Not Found", "Not found.").await;
            continue;
        }
        let params: HashMap<String, String> = url.query_pairs().into_owned().collect();
        if let Some(error) = params.get("error") {
            respond(
                &mut stream,
                "200 OK",
                "Authorization failed. You can close this window.",
            )
            .await;
            return Err(match params.get("error_description") {
                Some(d) => format!("authorization failed: {error} ({d})"),
                None => format!("authorization failed: {error}"),
            });
        }
        if params.get("state").map(String::as_str) != Some(state) {
            respond(
                &mut stream,
                "400 Bad Request",
                "Unexpected state; please try again.",
            )
            .await;
            return Err("the redirect carried the wrong `state` (possible CSRF); aborted".into());
        }
        let Some(code) = params.get("code").filter(|c| !c.is_empty()) else {
            respond(&mut stream, "400 Bad Request", "No authorization code.").await;
            return Err("the redirect carried no authorization code".into());
        };
        respond(
            &mut stream,
            "200 OK",
            "Signed in. You can close this window and return to SnapRequest.",
        )
        .await;
        return Ok(code.clone());
    }
}

/// Reads an HTTP request head and returns the request target of a `GET`.
async fn read_request_target(stream: &mut tokio::net::TcpStream) -> Option<String> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 2048];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > 16 * 1024 {
            return None;
        }
        let n = tokio::time::timeout(Duration::from_secs(10), stream.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let head = String::from_utf8_lossy(&buf);
    let mut line = head.lines().next()?.split(' ');
    match (line.next(), line.next()) {
        (Some("GET"), Some(target)) => Some(target.to_owned()),
        _ => None,
    }
}

async fn respond(stream: &mut tokio::net::TcpStream, status: &str, text: &str) {
    let body = format!(
        "<!DOCTYPE html><html><head><meta charset=\"utf-8\"><title>SnapRequest</title></head><body><p>{text}</p></body></html>"
    );
    let resp = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(resp.as_bytes()).await;
    let _ = stream.shutdown().await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Header, HttpVersion, Sizes, Timings};

    fn response(status: u16, body: &str) -> Response {
        Response {
            status,
            version: HttpVersion::Http11,
            url: String::new(),
            headers: vec![Header::new("content-type", "application/json")],
            body: bytes::Bytes::from(body.to_owned()),
            remote_addr: None,
            timings: Timings::default(),
            sizes: Sizes::default(),
            trace: vec![],
        }
    }

    fn at(secs: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(secs)
    }

    #[test]
    fn token_responses() {
        let t = parse_token_response(
            &response(
                200,
                r#"{"access_token":"a","token_type":"bearer","expires_in":"3600","refresh_token":"r","scope":"read"}"#,
            ),
            at(1000),
        )
        .unwrap();
        assert_eq!(t.expires_at, Some(4600));
        assert_eq!(t.refresh_token.as_deref(), Some("r"));
        assert!(t.is_fresh(at(4589)));
        assert!(!t.is_fresh(at(4590)));
        assert_eq!(t.expires_in(at(1600)), Some(3000));

        let no_expiry =
            parse_token_response(&response(200, r#"{"access_token":"a"}"#), at(0)).unwrap();
        assert_eq!(no_expiry.token_type, "Bearer");
        assert!(no_expiry.is_fresh(at(u64::from(u32::MAX))));

        assert_eq!(
            parse_token_response(
                &response(
                    400,
                    r#"{"error":"invalid_client","error_description":"bad secret"}"#
                ),
                at(0)
            )
            .unwrap_err(),
            "the token endpoint answered 400: invalid_client (bad secret)"
        );
        assert_eq!(
            parse_token_response(&response(502, "<html>"), at(0)).unwrap_err(),
            "the token endpoint answered 502"
        );
        assert!(
            parse_token_response(&response(200, r#"{"token":"x"}"#), at(0))
                .unwrap_err()
                .contains("without a usable token")
        );
    }

    /// RFC 7636 appendix B.
    #[test]
    fn pkce_s256() {
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let (v, c) = pkce();
        assert_eq!(v.len(), 43);
        assert_eq!(c, pkce_challenge(&v));
    }

    #[test]
    fn token_requests_authenticate_the_client() {
        let mut o = OAuth2::new(OAuth2Grant::Password, "https://t.test/token", "my app");
        o.client_secret = "s&cret".into();
        o.username = "u".into();
        o.password = "p".into();
        o.scope = "read write".into();
        o.params.push(KeyValue::new("audience", "api"));
        let base = Request::get("https://api.test/");

        let req = token_request(&o, grant_params(&o), true, &base);
        assert_eq!(
            req.auth,
            Auth::Basic {
                username: "my+app".into(),
                password: "s%26cret".into()
            }
        );
        let Body::Form { fields } = &req.body else {
            panic!("form body expected")
        };
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            ["grant_type", "username", "password", "scope", "audience"]
        );

        o.client_auth = ClientAuth::Body;
        let req = token_request(&o, refresh_params(&o, "r"), false, &base);
        assert!(req.auth.is_none());
        let Body::Form { fields } = &req.body else {
            panic!("form body expected")
        };
        let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "grant_type",
                "refresh_token",
                "scope",
                "client_id",
                "client_secret"
            ]
        );

        o.client_secret.clear();
        let req = token_request(&o, grant_params(&o), false, &base);
        assert!(req.auth.is_none());
        assert!(req.secrets().contains(&"p".to_owned()));
    }

    #[test]
    fn authorization_urls() {
        let mut o = OAuth2::new(OAuth2Grant::AuthorizationCode, "https://t/token", "app");
        let redirect = Url::parse("http://127.0.0.1:5000/cb").unwrap();
        assert!(authorization_url(&o, &redirect, "st", "ch").is_err());
        o.auth_url = Some("https://login.test/authorize?tenant=x".into());
        o.scope = "openid profile".into();
        let url = authorization_url(&o, &redirect, "st", "ch").unwrap();
        assert_eq!(
            url.as_str(),
            "https://login.test/authorize?tenant=x&response_type=code&client_id=app&redirect_uri=http%3A%2F%2F127.0.0.1%3A5000%2Fcb&scope=openid+profile&state=st&code_challenge=ch&code_challenge_method=S256"
        );
    }

    #[tokio::test]
    async fn redirect_uris_must_be_loopback() {
        let mut o = OAuth2::new(OAuth2Grant::AuthorizationCode, "https://t/token", "app");
        let (_l, uri) = redirect_listener(&o).await.unwrap();
        assert_ne!(uri.port(), Some(0));
        assert_eq!(uri.path(), "/callback");
        for bad in ["https://127.0.0.1/cb", "http://example.com/cb", "nope"] {
            o.redirect_uri = Some(bad.into());
            assert!(redirect_listener(&o).await.is_err(), "{bad}");
        }
    }

    #[tokio::test]
    async fn the_callback_delivers_the_code() {
        let o = OAuth2::new(OAuth2Grant::AuthorizationCode, "https://t/token", "app");
        let (listener, uri) = redirect_listener(&o).await.unwrap();
        let addr = format!("127.0.0.1:{}", uri.port().unwrap());
        let get = |target: &'static str, addr: String| async move {
            let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
            s.write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                .await
                .unwrap();
            let mut out = String::new();
            s.read_to_string(&mut out).await.unwrap();
            out
        };
        let browser = tokio::spawn(async move {
            let favicon = get("/favicon.ico", addr.clone()).await;
            let ok = get("/callback?code=c0de&state=s1", addr).await;
            (favicon, ok)
        });
        let code = wait_for_code(&listener, &uri, "s1").await.unwrap();
        assert_eq!(code, "c0de");
        let (favicon, ok) = browser.await.unwrap();
        assert!(favicon.starts_with("HTTP/1.1 404"));
        assert!(ok.starts_with("HTTP/1.1 200") && ok.contains("Signed in"));

        let (listener, uri) = redirect_listener(&o).await.unwrap();
        let addr = format!("127.0.0.1:{}", uri.port().unwrap());
        tokio::spawn(get(
            "/callback?error=access_denied&error_description=nope",
            addr,
        ));
        assert_eq!(
            wait_for_code(&listener, &uri, "s1").await.unwrap_err(),
            "authorization failed: access_denied (nope)"
        );

        let (listener, uri) = redirect_listener(&o).await.unwrap();
        let addr = format!("127.0.0.1:{}", uri.port().unwrap());
        tokio::spawn(get("/callback?code=x&state=other", addr));
        assert!(
            wait_for_code(&listener, &uri, "s1")
                .await
                .unwrap_err()
                .contains("wrong `state`")
        );
    }
}
