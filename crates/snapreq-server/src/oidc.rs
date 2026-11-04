//! Login through an OpenID Connect provider (PLAN P8.3, ADR-025).
//!
//! Authorization code flow with PKCE (`S256`), a `state` bound to the browser by a short-lived
//! cookie, and a `nonce` checked in the ID token. The ID token comes straight from the provider's
//! token endpoint, so — as OpenID Connect Core 3.1.3.7 allows — the connection to the provider
//! vouches for it instead of a signature check; its issuer, audience, expiry and nonce are checked.
//!
//! A provider identity (`iss` + `sub`) is linked to one account. The first login creates the
//! account (no password, no workspace); a name already taken by another account gets a suffix, so
//! an identity can never take over an existing local account. `allowed_domains` limits who may log
//! in to verified e-mail addresses of those domains.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use base64::Engine as _;
use rusqlite::{OptionalExtension, params};
use serde::Deserialize;
use serde_json::Value;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::accounts::User;
use crate::api::{ApiError, ApiResult, Shared};
use crate::config::OidcConfig;
use crate::store::{Store, db_err, now_ms};
use crate::{AppState, Error, Result};

/// Cookie that ties a login to the browser that started it.
const STATE_COOKIE: &str = "snapreq_oidc";
/// How long a started login may take.
const PENDING_TTL: Duration = Duration::from_secs(600);
/// Logins in progress at most (older ones are dropped).
const MAX_PENDING: usize = 1000;
/// Clock skew allowed when checking `exp`.
const SKEW_MS: i64 = 60_000;

/// The provider's endpoints (from its discovery document).
#[derive(Debug, Clone, Deserialize)]
struct Discovery {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
}

struct Pending {
    nonce: String,
    verifier: String,
    at: Instant,
}

/// OIDC state of the running server.
pub struct Oidc {
    pub config: OidcConfig,
    /// Talks to the provider (not limited by the outbound policy, which is for users' requests).
    client: snapreq_core::Client,
    discovery: tokio::sync::Mutex<Option<Discovery>>,
    pending: Mutex<HashMap<String, Pending>>,
}

impl std::fmt::Debug for Oidc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Oidc")
            .field("issuer", &self.config.issuer)
            .finish_non_exhaustive()
    }
}

/// 256 random bits, base64url.
fn random() -> Result<String> {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0u8; 32];
    ring::rand::SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| Error::Account("the system random generator failed".into()))?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn pkce_challenge(verifier: &str) -> String {
    let d = ring::digest::digest(&ring::digest::SHA256, verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(d.as_ref())
}

fn fail(message: impl Into<String>) -> Error {
    Error::Account(message.into())
}

impl Oidc {
    pub fn new(config: OidcConfig) -> Result<Self> {
        Ok(Self {
            config,
            client: snapreq_core::Client::new(snapreq_core::ClientConfig::default())?,
            discovery: tokio::sync::Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
        })
    }

    async fn get_json(&self, url: &str) -> Result<Value> {
        let resp = self
            .client
            .execute(&snapreq_core::Request::get(url))
            .await
            .map_err(|e| fail(format!("cannot reach the identity provider: {e}")))?;
        if resp.status != 200 {
            return Err(fail(format!("{url} answered {}", resp.status)));
        }
        serde_json::from_slice(&resp.body).map_err(|e| fail(format!("{url}: {e}")))
    }

    /// The discovery document, fetched once (and again after a failure).
    async fn discovery(&self) -> Result<Discovery> {
        let mut cached = self.discovery.lock().await;
        if let Some(d) = cached.as_ref() {
            return Ok(d.clone());
        }
        let url = format!(
            "{}/.well-known/openid-configuration",
            self.config.issuer.trim_end_matches('/')
        );
        let d: Discovery = serde_json::from_value(self.get_json(&url).await?)
            .map_err(|e| fail(format!("invalid discovery document: {e}")))?;
        if d.issuer.trim_end_matches('/') != self.config.issuer.trim_end_matches('/') {
            return Err(fail(format!(
                "the provider says its issuer is `{}`, not `{}`",
                d.issuer, self.config.issuer
            )));
        }
        *cached = Some(d.clone());
        Ok(d)
    }

    /// Starts a login: the provider URL to send the browser to, and the `state` for the cookie.
    async fn start(&self) -> Result<(String, String)> {
        let d = self.discovery().await?;
        let (state, nonce, verifier) = (random()?, random()?, random()?);
        let mut url = url::Url::parse(&d.authorization_endpoint)
            .map_err(|e| fail(format!("invalid authorization endpoint: {e}")))?;
        url.query_pairs_mut()
            .append_pair("response_type", "code")
            .append_pair("client_id", &self.config.client_id)
            .append_pair("redirect_uri", &self.config.redirect_url)
            .append_pair("scope", &self.config.scopes.join(" "))
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge", &pkce_challenge(&verifier))
            .append_pair("code_challenge_method", "S256");
        let mut pending = self.pending.lock().unwrap_or_else(|e| e.into_inner());
        pending.retain(|_, p| p.at.elapsed() < PENDING_TTL);
        if pending.len() >= MAX_PENDING
            && let Some(oldest) = pending
                .iter()
                .min_by_key(|(_, p)| p.at)
                .map(|(k, _)| k.clone())
        {
            pending.remove(&oldest);
        }
        pending.insert(
            state.clone(),
            Pending {
                nonce,
                verifier,
                at: Instant::now(),
            },
        );
        Ok((url.to_string(), state))
    }

    /// Finishes a login: exchanges the code and checks the ID token; returns its claims.
    async fn finish(&self, state: &str, code: &str) -> Result<Value> {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(state)
            .filter(|p| p.at.elapsed() < PENDING_TTL)
            .ok_or_else(|| fail("the login expired or was started elsewhere; try again"))?;
        let d = self.discovery().await?;
        let field = |name: &str, value: &str| snapreq_core::model::KeyValue {
            name: name.into(),
            value: value.into(),
            enabled: true,
        };
        let mut fields = vec![
            field("grant_type", "authorization_code"),
            field("code", code),
            field("redirect_uri", &self.config.redirect_url),
            field("client_id", &self.config.client_id),
            field("code_verifier", &pending.verifier),
        ];
        if let Some(secret) = &self.config.client_secret {
            fields.push(field("client_secret", secret));
        }
        let mut req = snapreq_core::Request::new(axum::http::Method::POST, &d.token_endpoint)
            .header("Accept", "application/json");
        req.body = snapreq_core::Body::Form { fields };
        let resp = self
            .client
            .execute(&req)
            .await
            .map_err(|e| fail(format!("cannot reach the token endpoint: {e}")))?;
        let doc: Value = serde_json::from_slice(&resp.body).unwrap_or(Value::Null);
        if resp.status != 200 {
            let why = doc["error_description"]
                .as_str()
                .or(doc["error"].as_str())
                .unwrap_or("no details");
            return Err(fail(format!(
                "the provider refused the login ({}: {why})",
                resp.status
            )));
        }
        let token = doc["id_token"]
            .as_str()
            .ok_or_else(|| fail("the provider sent no ID token"))?;
        let claims = id_token_claims(token)?;
        check_claims(
            &claims,
            &d.issuer,
            &self.config.client_id,
            &pending.nonce,
            now_ms(),
        )?;
        check_domain(&claims, &self.config.allowed_domains)?;
        Ok(claims)
    }
}

/// The payload of a JWT (not verified here, see the module documentation).
fn id_token_claims(token: &str) -> Result<Value> {
    let payload = token
        .split('.')
        .nth(1)
        .ok_or_else(|| fail("the ID token is not a JWT"))?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .map_err(|_| fail("the ID token is not a JWT"))?;
    serde_json::from_slice(&bytes).map_err(|_| fail("the ID token is not a JWT"))
}

fn check_claims(c: &Value, issuer: &str, client_id: &str, nonce: &str, now: i64) -> Result<()> {
    if c["iss"].as_str() != Some(issuer) {
        return Err(fail("the ID token comes from another issuer"));
    }
    let aud_ok = match &c["aud"] {
        Value::String(a) => a == client_id,
        Value::Array(list) => list.iter().any(|a| a.as_str() == Some(client_id)),
        _ => false,
    };
    if !aud_ok {
        return Err(fail("the ID token is for another client"));
    }
    let exp = c["exp"]
        .as_i64()
        .ok_or_else(|| fail("the ID token has no expiry"))?;
    if exp.saturating_mul(1000) + SKEW_MS < now {
        return Err(fail("the ID token has expired"));
    }
    if c["nonce"].as_str() != Some(nonce) {
        return Err(fail("the ID token does not belong to this login"));
    }
    if c["sub"].as_str().is_none_or(str::is_empty) {
        return Err(fail("the ID token names no subject"));
    }
    Ok(())
}

fn check_domain(c: &Value, allowed: &[String]) -> Result<()> {
    if allowed.is_empty() {
        return Ok(());
    }
    let email = c["email"].as_str().unwrap_or_default().to_ascii_lowercase();
    let verified =
        c["email_verified"].as_bool() == Some(true) || c["email_verified"].as_str() == Some("true");
    let domain = email.rsplit_once('@').map(|(_, d)| d).unwrap_or_default();
    if verified && allowed.iter().any(|a| a == domain) {
        Ok(())
    } else {
        Err(fail(
            "this server only accepts verified e-mail addresses of its allowed domains",
        ))
    }
}

/// A user name from the claims: `preferred_username`, the e-mail's local part, or `user`;
/// characters that names cannot have become `-`.
fn base_name(c: &Value) -> String {
    let raw = c["preferred_username"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
        .or_else(|| {
            c["email"]
                .as_str()
                .and_then(|e| e.split('@').next())
                .map(str::to_owned)
        })
        .unwrap_or_default();
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '_' | '-' | '.' | '@') {
                ch
            } else {
                '-'
            }
        })
        .take(56)
        .collect();
    if cleaned.is_empty() {
        "user".into()
    } else {
        cleaned
    }
}

impl Store {
    /// The account of a provider identity, created on its first login.
    pub async fn oidc_user(&self, issuer: &str, claims: &Value) -> Result<(User, bool)> {
        let issuer = issuer.to_owned();
        let subject = claims["sub"].as_str().unwrap_or_default().to_owned();
        let email = claims["email"].as_str().map(str::to_owned);
        let base = base_name(claims);
        self.with(move |c| {
            let tx = c.transaction().map_err(db_err)?;
            let linked: Option<User> = tx
                .query_row(
                    "SELECT u.id, u.username, u.created_at FROM identities i JOIN users u ON u.id = i.user_id
                     WHERE i.issuer = ?1 AND i.subject = ?2",
                    params![issuer, subject],
                    |r| Ok(User { id: r.get(0)?, username: r.get(1)?, created_at: r.get(2)? }),
                )
                .optional()
                .map_err(db_err)?;
            if let Some(user) = linked {
                return Ok((user, false));
            }
            let taken = |name: &str| -> Result<bool> {
                tx.query_row(
                    "SELECT EXISTS (SELECT 1 FROM users WHERE username = ?1)",
                    [name],
                    |r| r.get(0),
                )
                .map_err(db_err)
            };
            let mut name = base.clone();
            let mut n = 1;
            while taken(&name)? {
                n += 1;
                if n > 1000 {
                    return Err(fail("no free user name for this identity"));
                }
                name = format!("{base}-{n}");
            }
            let created_at = now_ms();
            // `!` is not a PHC string: no password can match it.
            tx.execute(
                "INSERT INTO users (username, password_hash, created_at) VALUES (?1, '!', ?2)",
                params![name, created_at],
            )
            .map_err(db_err)?;
            let id = tx.last_insert_rowid();
            tx.execute(
                "INSERT INTO identities (issuer, subject, user_id, email, created_at) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![issuer, subject, id, email, created_at],
            )
            .map_err(db_err)?;
            tx.execute(
                "INSERT INTO members (workspace_id, user_id, role)
                 SELECT 'default', ?1, 'owner' WHERE NOT EXISTS (SELECT 1 FROM members WHERE workspace_id = 'default')",
                [id],
            )
            .map_err(db_err)?;
            tx.commit().map_err(db_err)?;
            Ok((User { id, username: name, created_at }, true))
        })
        .await
    }
}

// --- Endpoints ---------------------------------------------------------------

pub fn router() -> OpenApiRouter<std::sync::Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(start))
        .routes(routes!(callback))
}

fn redirect(to: &str, cookies: Vec<HeaderValue>) -> Response {
    let mut resp = (StatusCode::FOUND, [(header::LOCATION, to.to_owned())]).into_response();
    for c in cookies {
        resp.headers_mut().append(header::SET_COOKIE, c);
    }
    resp
}

/// Back to the UI's login page with a message.
fn back_with_error(message: &str) -> Response {
    let q: String = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("login_error", message)
        .finish();
    redirect(
        &format!("/?{q}"),
        vec![HeaderValue::from_static(
            "snapreq_oidc=; Path=/api/v1/auth/oidc; HttpOnly; SameSite=Lax; Max-Age=0",
        )],
    )
}

fn configured(state: &AppState) -> ApiResult<&Oidc> {
    state
        .oidc
        .as_ref()
        .ok_or_else(|| ApiError::not_found("single sign-on is not configured"))
}

#[utoipa::path(get, path = "/auth/oidc/start", tag = "auth", responses((status = 302, description = "To the identity provider's login page"), (status = 404, description = "Single sign-on is not configured")))]
/// `GET /api/v1/auth/oidc/start`: the login button's target.
async fn start(State(state): Shared) -> ApiResult<Response> {
    let oidc = configured(&state)?;
    let (url, st) = match oidc.start().await {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("single sign-on cannot start: {e}");
            return Ok(back_with_error(&e.to_string()));
        }
    };
    let secure = if state.config.auth.secure_cookies {
        "; Secure"
    } else {
        ""
    };
    // `Lax`: the provider sends the browser back with a top-level cross-site redirect.
    let cookie = HeaderValue::from_str(&format!(
        "{STATE_COOKIE}={st}; Path=/api/v1/auth/oidc; HttpOnly; SameSite=Lax; Max-Age={}{secure}",
        PENDING_TTL.as_secs()
    ))
    .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(redirect(&url, vec![cookie]))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[into_params(parameter_in = Query)]
struct CallbackParams {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    error_description: Option<String>,
}

#[utoipa::path(get, path = "/auth/oidc/callback", tag = "auth", params(CallbackParams), responses((status = 302, description = "Logged in (session cookie set) and back to the UI, or back to the login page with `login_error`")))]
/// `GET /api/v1/auth/oidc/callback`: where the provider sends the browser back.
async fn callback(
    State(state): Shared,
    ip: Option<axum::Extension<crate::security::ClientIp>>,
    headers: HeaderMap,
    Query(p): Query<CallbackParams>,
) -> ApiResult<Response> {
    let oidc = configured(&state)?;
    if let Some(e) = p.error {
        let why = p.error_description.unwrap_or(e);
        return Ok(back_with_error(&format!(
            "the provider refused the login: {why}"
        )));
    }
    let (Some(code), Some(st)) = (p.code, p.state) else {
        return Ok(back_with_error("the provider sent no code"));
    };
    if crate::auth::cookie(&headers, STATE_COOKIE) != Some(st.as_str()) {
        return Ok(back_with_error(
            "the login was started in another browser or has expired; try again",
        ));
    }
    let claims = match oidc.finish(&st, &code).await {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("single sign-on failed: {e}");
            return Ok(back_with_error(&e.to_string()));
        }
    };
    let issuer = claims["iss"].as_str().unwrap_or_default().to_owned();
    let (user, created) = state.store.oidc_user(&issuer, &claims).await?;
    if created {
        tracing::info!(username = %user.username, "account created by single sign-on");
    }
    tracing::info!(username = %user.username, "logged in with single sign-on");
    let ip = ip.map_or(crate::security::ClientIp(None), |axum::Extension(i)| i);
    let event = |action| crate::security::Audit {
        user_id: Some(user.id),
        username: Some(user.username.clone()),
        ..crate::security::Audit::new(action)
            .ip(ip)
            .target(issuer.clone())
    };
    if created {
        state.store.audit(event("user.created_by_oidc")).await;
    }
    state.store.audit(event("auth.oidc_login")).await;
    let ttl = state.config.auth.session_ttl;
    let secret = state.store.create_session(user.id, ttl).await?;
    let session = crate::auth::session_cookie(&state, &secret, ttl.as_secs())?;
    let clear = HeaderValue::from_static(
        "snapreq_oidc=; Path=/api/v1/auth/oidc; HttpOnly; SameSite=Lax; Max-Age=0",
    );
    Ok(redirect("/", vec![session, clear]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn jwt(claims: &Value) -> String {
        let e = |v: &Value| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(v.to_string());
        format!("{}.{}.sig", e(&json!({ "alg": "RS256" })), e(claims))
    }

    #[test]
    fn id_tokens_are_checked() {
        let now = 1_800_000_000_000;
        let good = json!({ "iss": "https://idp", "aud": ["x", "snapreq"], "exp": now / 1000 + 60, "nonce": "n", "sub": "42" });
        let c = id_token_claims(&jwt(&good)).unwrap();
        assert!(check_claims(&c, "https://idp", "snapreq", "n", now).is_ok());
        let bad = |patch: Value, why: &str| {
            let mut c = good.clone();
            for (k, v) in patch.as_object().unwrap() {
                c[k] = v.clone();
            }
            let err = check_claims(&c, "https://idp", "snapreq", "n", now).unwrap_err();
            assert!(err.to_string().contains(why), "{err}");
        };
        bad(json!({ "iss": "https://evil" }), "another issuer");
        bad(json!({ "aud": "other" }), "another client");
        bad(json!({ "exp": now / 1000 - 120 }), "expired");
        bad(json!({ "nonce": "replayed" }), "this login");
        bad(json!({ "sub": "" }), "no subject");
        assert!(id_token_claims("not-a-jwt").is_err());
    }

    #[test]
    fn domains_and_names() {
        let allowed = vec!["example.com".to_owned()];
        let ok = json!({ "email": "Ana@Example.com", "email_verified": true });
        assert!(check_domain(&ok, &allowed).is_ok());
        assert!(
            check_domain(
                &json!({ "email": "ana@example.com", "email_verified": false }),
                &allowed
            )
            .is_err()
        );
        assert!(
            check_domain(
                &json!({ "email": "ana@evil.com", "email_verified": true }),
                &allowed
            )
            .is_err()
        );
        assert!(
            check_domain(
                &json!({ "email": "ana@sub.example.com", "email_verified": true }),
                &allowed
            )
            .is_err()
        );
        assert!(check_domain(&json!({}), &[]).is_ok());
        assert_eq!(
            base_name(&json!({ "preferred_username": "Ana López" })),
            "Ana-López"
        );
        assert_eq!(base_name(&json!({ "email": "bo@example.com" })), "bo");
        assert_eq!(base_name(&json!({})), "user");
        assert_eq!(
            pkce_challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
    }
}
