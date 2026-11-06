//! Authentication (PLAN P8.1, ADR-023): who is calling, and the `/api/v1/auth` endpoints.
//!
//! Without accounts (`auth.mode = "none"`, the default on loopback addresses) every caller is
//! the server's single user. With accounts, every API route except the public ones below needs
//! either the session cookie set by `POST /auth/login` or an API token
//! (`Authorization: Bearer snapreq_pat_…`).
//!
//! Requests authenticated by the cookie that change something (any method but `GET`, `HEAD`,
//! `OPTIONS`) must carry the `X-SnapReq-CSRF` header; browsers only let pages of other origins
//! send custom headers after a CORS preflight, which this server never grants. The WebSocket
//! handshake must come from this server's own origin when the browser says where it comes from.

use std::sync::Arc;

use axum::extract::{FromRequestParts, Path as UrlPath, Request, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::Deserialize;
use serde_json::{Value, json};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AppState;
use crate::accounts::{ApiToken, User};
use crate::api::{ApiError, ApiResult, ErrorBody, Shared};
use crate::config::AuthMode;
use crate::security::{Audit, ClientIp, too_many};
use crate::store::Owner;

/// Name of the session cookie.
pub const SESSION_COOKIE: &str = "snapreq_session";

/// Header that cookie-authenticated requests changing something must send.
pub const CSRF_HEADER: &str = "x-snapreq-csrf";

/// How the caller was recognised.
#[derive(Clone)]
pub enum Via {
    /// A server without accounts: everyone is its single user.
    Local,
    /// The session cookie (its secret, needed to end the session).
    Session(String),
    /// An API token (its secret, to check it is still valid on a long-lived WebSocket).
    Token(String),
}

impl std::fmt::Debug for Via {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Local => "Local",
            Self::Session(_) => "Session(<redacted>)",
            Self::Token(_) => "Token(<redacted>)",
        })
    }
}

/// The caller of a request; handlers take it as an extractor.
#[derive(Debug, Clone)]
pub struct Identity {
    /// `None` on a server without accounts.
    pub user: Option<User>,
    pub via: Via,
    /// The client's address (for the audit log).
    pub ip: Option<std::net::IpAddr>,
}

impl Identity {
    fn local() -> Self {
        Self {
            user: None,
            via: Via::Local,
            ip: None,
        }
    }

    /// Whose history and runs this caller sees.
    pub fn owner(&self) -> Owner {
        self.user.as_ref().map(|u| u.id)
    }

    /// Whether the session or token is still valid (it may have ended since a WebSocket
    /// was opened with it).
    pub async fn still_valid(&self, state: &AppState) -> crate::Result<bool> {
        let found = match &self.via {
            Via::Local => return Ok(true),
            Via::Session(secret) => state.store.session_user(secret).await?,
            Via::Token(secret) => state.store.token_user(secret).await?,
        };
        Ok(found.is_some_and(|u| Some(u.id) == self.owner()))
    }

    /// The account, or a conflict on a server without accounts.
    pub(crate) fn account(&self) -> ApiResult<&User> {
        self.user.as_ref().ok_or_else(accounts_off)
    }
}

impl<S: Send + Sync> FromRequestParts<S> for Identity {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .extensions
            .get::<Self>()
            .cloned()
            .ok_or_else(|| unauthorized("log in or send an API token"))
    }
}

fn unauthorized(message: &str) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "unauthorized", message)
}

fn forbidden(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::FORBIDDEN, "forbidden", message)
}

fn accounts_off() -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "conflict",
        "this server has no accounts (auth mode `none`)",
    )
}

/// API routes anyone may call.
fn is_public(path: &str) -> bool {
    matches!(
        path,
        "/api/v1/version"
            | "/api/v1/openapi.json"
            | "/api/v1/auth/status"
            | "/api/v1/auth/login"
            | "/api/v1/auth/oidc/start"
            | "/api/v1/auth/oidc/callback"
    )
}

fn is_api(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

/// The value of cookie `name`.
pub(crate) fn cookie<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(k, _)| *k == name)
        .map(|(_, v)| v.trim())
        .filter(|v| !v.is_empty())
}

fn bearer(headers: &HeaderMap) -> Option<&str> {
    let value = headers.get(header::AUTHORIZATION)?.to_str().ok()?;
    let (scheme, token) = value.split_once(' ')?;
    scheme
        .eq_ignore_ascii_case("bearer")
        .then(|| token.trim())
        .filter(|t| !t.is_empty())
}

/// Whether a browser's `Origin` (if any) is this server: its host matches `Host` or, behind a
/// proxy, `X-Forwarded-Host` (pages cannot set either on a WebSocket handshake).
fn same_origin(headers: &HeaderMap) -> bool {
    let Some(origin) = headers.get(header::ORIGIN).and_then(|v| v.to_str().ok()) else {
        return true; // not a browser
    };
    let Some((_, host)) = origin.split_once("://") else {
        return false; // `null` and other opaque origins
    };
    ["host", "x-forwarded-host"].iter().any(|name| {
        headers
            .get(*name)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(',').next())
            .is_some_and(|v| v.trim().eq_ignore_ascii_case(host))
    })
}

/// Who is calling: `Ok(None)` when no credentials were sent, an error for bad ones.
async fn identify(state: &AppState, headers: &HeaderMap) -> ApiResult<Option<Identity>> {
    if state.config.auth.mode == AuthMode::None {
        return Ok(Some(Identity::local()));
    }
    if let Some(token) = bearer(headers) {
        return match state.store.token_user(token).await? {
            Some(user) => Ok(Some(Identity {
                user: Some(user),
                via: Via::Token(token.to_owned()),
                ip: None,
            })),
            None => Err(unauthorized("invalid or expired API token")),
        };
    }
    if let Some(secret) = cookie(headers, SESSION_COOKIE) {
        // An expired session is the same as none: public routes still work.
        return Ok(state
            .store
            .session_user(secret)
            .await?
            .map(|user| Identity {
                user: Some(user),
                via: Via::Session(secret.to_owned()),
                ip: None,
            }));
    }
    Ok(None)
}

/// Middleware in front of every route: identifies the caller and enforces the rules in the
/// module documentation.
pub async fn guard(State(state): State<Arc<AppState>>, mut req: Request, next: Next) -> Response {
    let path = req.uri().path().to_owned();
    if !is_api(&path) {
        return next.run(req).await; // the UI's files and `/healthz`
    }
    // A server without a login answers only to names that cannot be rebound to another site.
    if state.config.auth.mode == AuthMode::None
        && !state.config.allow_remote
        && !crate::security::host_allowed(
            req.headers()
                .get(axum::http::header::HOST)
                .and_then(|v| v.to_str().ok()),
            &state.config.auth.allowed_hosts,
        )
    {
        tracing::warn!("refused a request for an unknown host name");
        return forbidden(
            "unknown host name: a server without accounts only answers to localhost and loopback addresses (add others to auth.allowed_hosts)",
        )
        .into_response();
    }
    let ip = crate::security::ClientIp(
        req.extensions()
            .get::<axum::extract::ConnectInfo<std::net::SocketAddr>>()
            .map(|c| c.0.ip()),
    );
    req.extensions_mut().insert(ip);
    let bearer_sent = bearer(req.headers()).is_some();
    if bearer_sent && let Some(left) = state.limits.token_ip.blocked(&ip.key()) {
        return crate::security::too_many(left, "unknown API tokens");
    }
    if (path == "/api/v1/ws" || path.starts_with("/api/v1/ws/")) && !same_origin(req.headers()) {
        tracing::warn!("refused a WebSocket from another origin");
        return forbidden("WebSocket connections must come from this server's pages")
            .into_response();
    }
    let identity = match identify(&state, req.headers()).await {
        Ok(i) => i.map(|i| Identity { ip: ip.0, ..i }),
        Err(e) => {
            if bearer_sent && e.status == StatusCode::UNAUTHORIZED {
                state.limits.token_ip.fail(&ip.key());
            }
            return e.into_response();
        }
    };
    let changes = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    let by_cookie = matches!(
        identity,
        Some(Identity {
            via: Via::Session(_),
            ..
        })
    );
    let logging_in = path == "/api/v1/auth/login" && state.config.auth.mode == AuthMode::Accounts;
    if changes && (by_cookie || logging_in) && !req.headers().contains_key(CSRF_HEADER) {
        return forbidden(format!(
            "requests from the browser must send the `{CSRF_HEADER}` header"
        ))
        .into_response();
    }
    match identity {
        Some(identity) => {
            req.extensions_mut().insert(identity);
        }
        None if is_public(&path) => {}
        None => return unauthorized("log in or send an API token").into_response(),
    }
    next.run(req).await
}

/// The `/api/v1/auth` routes.
pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(status))
        .routes(routes!(login))
        .routes(routes!(logout))
        .routes(routes!(change_password))
        .routes(routes!(list_tokens, create_token))
        .routes(routes!(delete_token))
}

/// `{ mode, user }`
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct AuthStatus {
    /// `none` (no login) or `accounts`.
    pub mode: String,
    /// The logged-in user; `null` when not logged in or without accounts.
    pub user: Option<User>,
    /// Single sign-on, if configured: the login button's label (`GET /auth/oidc/start` logs in).
    pub oidc: Option<OidcInfo>,
}

#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct OidcInfo {
    pub name: String,
}

fn oidc_info(state: &AppState) -> Option<OidcInfo> {
    state.oidc.as_ref().map(|o| OidcInfo {
        name: o.config.name.clone(),
    })
}

#[utoipa::path(get, path = "/auth/status", tag = "auth", responses((status = 200, description = "Whether the server has accounts, and who is logged in", body = AuthStatus)))]
async fn status(State(state): Shared, identity: Option<Extension<Identity>>) -> Json<AuthStatus> {
    Json(AuthStatus {
        mode: state.config.auth.mode.as_str().to_owned(),
        user: identity.and_then(|Extension(i)| i.user),
        oidc: oidc_info(&state),
    })
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct LoginBody {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for LoginBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoginBody")
            .field("username", &self.username)
            .finish_non_exhaustive()
    }
}

pub(crate) fn session_cookie(
    state: &AppState,
    value: &str,
    max_age_secs: u64,
) -> ApiResult<HeaderValue> {
    let secure = if state.config.auth.secure_cookies {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={value}; Path=/; HttpOnly; SameSite=Strict; Max-Age={max_age_secs}{secure}"
    ))
    .map_err(|e| ApiError::internal(e.to_string()))
}

#[utoipa::path(post, path = "/auth/login", tag = "auth", request_body = LoginBody, responses((status = 200, description = "Logged in; sets the session cookie", body = AuthStatus), (status = 401, description = "Wrong user name or password", body = ErrorBody), (status = 409, description = "The server has no accounts", body = ErrorBody)))]
/// `POST /api/v1/auth/login`: checks the password and starts a session (an HttpOnly cookie).
async fn login(
    State(state): Shared,
    ip: Option<Extension<ClientIp>>,
    Json(body): Json<LoginBody>,
) -> ApiResult<Response> {
    if state.config.auth.mode != AuthMode::Accounts {
        return Err(accounts_off());
    }
    let ip = ip.map_or(ClientIp(None), |Extension(i)| i);
    let user_key = body.username.trim().to_lowercase();
    let limits = &state.limits;
    if let Some(left) = limits
        .login_user
        .blocked(&user_key)
        .or_else(|| limits.login_ip.blocked(&ip.key()))
    {
        return Ok(too_many(left, "failed logins"));
    }
    let Some(user) = state
        .store
        .check_login(&body.username, &body.password)
        .await?
    else {
        limits.login_user.fail(&user_key);
        limits.login_ip.fail(&ip.key());
        tracing::warn!(username = %body.username, "failed login");
        state
            .store
            .audit(
                Audit::new("auth.login_failed")
                    .target(body.username.clone())
                    .ip(ip),
            )
            .await;
        return Err(unauthorized("wrong user name or password"));
    };
    limits.login_user.reset(&user_key);
    let ttl = state.config.auth.session_ttl;
    let secret = state.store.create_session(user.id, ttl).await?;
    tracing::info!(username = %user.username, "logged in");
    state
        .store
        .audit(Audit {
            user_id: Some(user.id),
            username: Some(user.username.clone()),
            ..Audit::new("auth.login").ip(ip)
        })
        .await;
    let cookie = session_cookie(&state, &secret, ttl.as_secs())?;
    Ok((
        [(header::SET_COOKIE, cookie)],
        Json(AuthStatus {
            mode: AuthMode::Accounts.as_str().to_owned(),
            user: Some(user),
            oidc: oidc_info(&state),
        }),
    )
        .into_response())
}

#[utoipa::path(post, path = "/auth/logout", tag = "auth", responses((status = 204, description = "The session is ended and its cookie removed")))]
/// `POST /api/v1/auth/logout`
async fn logout(State(state): Shared, identity: Identity) -> ApiResult<Response> {
    if let Via::Session(secret) = &identity.via {
        state.store.delete_session(secret).await?;
        state
            .store
            .audit(Audit::new("auth.logout").by(&identity))
            .await;
    }
    let cookie = session_cookie(&state, "", 0)?;
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response())
}

#[derive(Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct PasswordBody {
    pub current: String,
    pub new: String,
}

impl std::fmt::Debug for PasswordBody {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PasswordBody { .. }")
    }
}

#[utoipa::path(post, path = "/auth/password", tag = "auth", request_body = PasswordBody, responses((status = 204, description = "Changed; the user's other sessions are ended (API tokens stay valid)"), (status = 403, description = "The current password is wrong", body = ErrorBody), (status = 409, description = "The server has no accounts", body = ErrorBody), (status = 422, description = "The new password is not accepted", body = ErrorBody)))]
/// `POST /api/v1/auth/password`: changes the caller's password.
async fn change_password(
    State(state): Shared,
    identity: Identity,
    Json(body): Json<PasswordBody>,
) -> ApiResult<StatusCode> {
    let user = identity.account()?;
    if state
        .store
        .check_login(&user.username, &body.current)
        .await?
        .is_none()
    {
        state
            .store
            .audit(Audit::new("auth.password_change_failed").by(&identity))
            .await;
        return Err(forbidden("the current password is wrong"));
    }
    let keep = match &identity.via {
        Via::Session(secret) => Some(secret.as_str()),
        _ => None,
    };
    state
        .store
        .set_password(&user.username, &body.new, keep)
        .await?;
    tracing::info!(username = %user.username, "password changed");
    state
        .store
        .audit(Audit::new("auth.password_changed").by(&identity))
        .await;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, utoipa::ToSchema)]
pub struct TokenList {
    pub tokens: Vec<ApiToken>,
}

#[utoipa::path(get, path = "/auth/tokens", tag = "auth", responses((status = 200, description = "The caller's API tokens, newest first", body = TokenList), (status = 409, description = "The server has no accounts", body = ErrorBody)))]
async fn list_tokens(State(state): Shared, identity: Identity) -> ApiResult<Json<Value>> {
    let user = identity.account()?;
    Ok(Json(
        json!({ "tokens": state.store.tokens(user.id).await? }),
    ))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct TokenBody {
    /// What the token is for, e.g. `ci`.
    pub name: String,
    /// Days until it expires; absent: never.
    pub expires_in_days: Option<u64>,
}

/// A new token: `secret` is shown only in this response.
#[derive(Debug, utoipa::ToSchema)]
pub struct NewToken {
    pub token: ApiToken,
    pub secret: String,
}

#[utoipa::path(post, path = "/auth/tokens", tag = "auth", request_body = TokenBody, responses((status = 201, description = "The token, with its secret (shown once)", body = NewToken), (status = 409, description = "The server has no accounts", body = ErrorBody), (status = 422, description = "Invalid name or expiry", body = ErrorBody)))]
/// `POST /api/v1/auth/tokens`: creates an API token for the caller.
async fn create_token(
    State(state): Shared,
    identity: Identity,
    Json(body): Json<TokenBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let user = identity.account()?;
    let ttl = match body.expires_in_days {
        None => None,
        Some(d @ 1..=3650) => Some(std::time::Duration::from_secs(d * 24 * 3600)),
        Some(_) => {
            return Err(ApiError::invalid(
                "`expires_in_days` must be between 1 and 3650",
            ));
        }
    };
    let (token, secret) = state.store.create_token(user.id, &body.name, ttl).await?;
    tracing::info!(username = %user.username, token = token.id, "API token created");
    state
        .store
        .audit(
            Audit::new("auth.token_created")
                .by(&identity)
                .target(format!("{} ({})", token.name, token.id)),
        )
        .await;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "token": token, "secret": secret })),
    ))
}

#[utoipa::path(delete, path = "/auth/tokens/{id}", tag = "auth", params(("id" = i64, Path, description = "Token id")), responses((status = 204, description = "Revoked"), (status = 404, description = "Not found", body = ErrorBody), (status = 409, description = "The server has no accounts", body = ErrorBody)))]
/// `DELETE /api/v1/auth/tokens/{id}`: revokes one of the caller's tokens.
async fn delete_token(
    State(state): Shared,
    identity: Identity,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<StatusCode> {
    let user = identity.account()?;
    if state.store.delete_token(user.id, id).await? {
        tracing::info!(username = %user.username, token = id, "API token revoked");
        state
            .store
            .audit(
                Audit::new("auth.token_revoked")
                    .by(&identity)
                    .target(id.to_string()),
            )
            .await;
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(ApiError::not_found(format!("no token {id}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&'static str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.append(*k, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn cookies_and_bearer_tokens() {
        let h = headers(&[("cookie", "a=1; snapreq_session=abc ; b=2")]);
        assert_eq!(cookie(&h, SESSION_COOKIE), Some("abc"));
        assert_eq!(
            cookie(&headers(&[("cookie", "snapreq_session=")]), SESSION_COOKIE),
            None
        );
        let h = headers(&[("cookie", "x=1"), ("cookie", "snapreq_session=two")]);
        assert_eq!(cookie(&h, SESSION_COOKIE), Some("two"));
        assert_eq!(
            bearer(&headers(&[("authorization", "Bearer snapreq_pat_x")])),
            Some("snapreq_pat_x")
        );
        assert_eq!(bearer(&headers(&[("authorization", "Basic abc")])), None);
    }

    #[test]
    fn websocket_origins() {
        assert!(same_origin(&headers(&[("host", "localhost:7878")])));
        assert!(same_origin(&headers(&[
            ("host", "localhost:7878"),
            ("origin", "http://localhost:7878")
        ])));
        assert!(!same_origin(&headers(&[
            ("host", "localhost:7878"),
            ("origin", "http://localhost:3000")
        ])));
        assert!(!same_origin(&headers(&[
            ("host", "localhost:7878"),
            ("origin", "null")
        ])));
        assert!(same_origin(&headers(&[
            ("host", "127.0.0.1:7878"),
            ("x-forwarded-host", "snapreq.example.com"),
            ("origin", "https://snapreq.example.com")
        ])));
    }

    #[test]
    fn public_routes() {
        assert!(is_public("/api/v1/auth/login"));
        assert!(!is_public("/api/v1/history"));
        assert!(!is_api("/healthz"));
        assert!(is_api("/api/v1/ws"));
    }
}
