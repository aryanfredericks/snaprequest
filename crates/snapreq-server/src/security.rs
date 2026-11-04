//! Hardening (PLAN P8.5, ADR-027): rate limits on failed logins and bad tokens, security headers,
//! a `Host` check against DNS rebinding, and the audit log.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::{Query, Request, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rusqlite::params;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::api::{ApiError, ApiResult, Shared};
use crate::auth::Identity;
use crate::store::{Owner, Store, db_err, now_ms};
use crate::{AppState, Result};

// --- Rate limits -----------------------------------------------------------------

/// Counts failures per key in a fixed window; a key with `max` failures is blocked until its
/// window ends.
#[derive(Debug)]
pub struct Limiter {
    max: u32,
    window: Duration,
    entries: Mutex<HashMap<String, (u32, Instant)>>,
}

/// Keys kept at most (expired ones are dropped first, then everything: an attacker cannot make
/// the server run out of memory, at worst reset the counters).
const MAX_KEYS: usize = 100_000;

impl Limiter {
    pub fn new(max: u32, window: Duration) -> Self {
        Self {
            max,
            window,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// How long `key` is still blocked.
    pub fn blocked(&self, key: &str) -> Option<Duration> {
        let entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let (count, start) = entries.get(key)?;
        let left = self.window.checked_sub(start.elapsed())?;
        (*count >= self.max).then_some(left)
    }

    pub fn fail(&self, key: &str) {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        if entries.len() >= MAX_KEYS {
            let window = self.window;
            entries.retain(|_, (_, at)| at.elapsed() < window);
            if entries.len() >= MAX_KEYS {
                entries.clear();
            }
        }
        let e = entries.entry(key.to_owned()).or_insert((0, Instant::now()));
        if e.1.elapsed() >= self.window {
            *e = (0, Instant::now());
        }
        e.0 += 1;
    }

    pub fn reset(&self, key: &str) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(key);
    }
}

/// The server's rate limits.
#[derive(Debug)]
pub struct Limits {
    /// Failed logins per user name.
    pub login_user: Limiter,
    /// Failed logins per client address.
    pub login_ip: Limiter,
    /// Unknown API tokens per client address.
    pub token_ip: Limiter,
}

impl Default for Limits {
    fn default() -> Self {
        let window = Duration::from_secs(15 * 60);
        Self {
            login_user: Limiter::new(10, window),
            login_ip: Limiter::new(50, window),
            token_ip: Limiter::new(50, window),
        }
    }
}

/// 429 with `Retry-After`.
pub fn too_many(left: Duration, what: &str) -> Response {
    let minutes = left.as_secs().div_ceil(60).max(1);
    let mut resp = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        format!("too many {what}; try again in {minutes} minute(s)"),
    )
    .into_response();
    if let Ok(v) = HeaderValue::from_str(&left.as_secs().max(1).to_string()) {
        resp.headers_mut().insert(header::RETRY_AFTER, v);
    }
    resp
}

/// The client's address, put into every request's extensions by the guard.
#[derive(Debug, Clone, Copy)]
pub struct ClientIp(pub Option<IpAddr>);

impl ClientIp {
    pub fn key(&self) -> String {
        self.0.map_or_else(|| "unknown".into(), |ip| ip.to_string())
    }
}

// --- Headers ---------------------------------------------------------------------

/// Adds security headers to every response.
pub async fn headers(State(state): State<Arc<AppState>>, req: Request, next: Next) -> Response {
    let api = req.uri().path().starts_with("/api/");
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    let set = |h: &mut HeaderMap, name: header::HeaderName, value: &'static str| {
        if !h.contains_key(&name) {
            h.insert(name, HeaderValue::from_static(value));
        }
    };
    set(h, header::X_CONTENT_TYPE_OPTIONS, "nosniff");
    set(h, header::X_FRAME_OPTIONS, "DENY");
    set(h, header::REFERRER_POLICY, "no-referrer");
    set(
        h,
        header::HeaderName::from_static("cross-origin-opener-policy"),
        "same-origin",
    );
    set(
        h,
        header::HeaderName::from_static("cross-origin-resource-policy"),
        "same-origin",
    );
    if api {
        set(h, header::CACHE_CONTROL, "no-store");
        set(
            h,
            header::CONTENT_SECURITY_POLICY,
            "default-src 'none'; frame-ancestors 'none'",
        );
    }
    if state.config.auth.secure_cookies {
        set(h, header::STRICT_TRANSPORT_SECURITY, "max-age=31536000");
    }
    resp
}

// --- Host check ------------------------------------------------------------------

/// The host name of a `Host` header (without the port).
fn host_name(host: &str) -> &str {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split(']').next().unwrap_or_default();
    }
    match host.rsplit_once(':') {
        Some((name, port)) if !name.contains(':') && port.chars().all(|c| c.is_ascii_digit()) => {
            name
        }
        _ => host,
    }
}

/// Whether a server without accounts may answer a request for `host`: loopback names and
/// addresses, and `allowed_hosts`. Other names mean a page used DNS rebinding to reach it.
pub fn host_allowed(host: Option<&str>, allowed: &[String]) -> bool {
    let Some(host) = host else { return true }; // HTTP/1.0 clients
    let name = host_name(host.trim())
        .trim_end_matches('.')
        .to_ascii_lowercase();
    name == "localhost"
        || name.ends_with(".localhost")
        || name.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
        || allowed.iter().any(|a| a.eq_ignore_ascii_case(&name))
}

// --- Audit log -------------------------------------------------------------------

/// How long audit events are kept.
pub const AUDIT_DAYS: u64 = 365;

/// A recorded action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, utoipa::ToSchema)]
pub struct AuditEvent {
    pub id: i64,
    /// Unix milliseconds.
    pub at: i64,
    /// Who did it; `null` for failed logins and the command line.
    pub username: Option<String>,
    pub workspace: Option<String>,
    /// E.g. `auth.login`, `workspace.member_set`, `secret.set`.
    pub action: String,
    pub target: Option<String>,
    /// The client's address.
    pub ip: Option<String>,
}

/// An action to record.
#[derive(Debug, Clone, Default)]
pub struct Audit {
    pub user_id: Option<i64>,
    pub username: Option<String>,
    pub workspace: Option<String>,
    pub action: &'static str,
    pub target: Option<String>,
    pub ip: Option<IpAddr>,
}

impl Audit {
    pub fn new(action: &'static str) -> Self {
        Self {
            action,
            ..Self::default()
        }
    }

    /// Done by `identity` (its user and address).
    pub fn by(mut self, identity: &Identity) -> Self {
        if let Some(u) = &identity.user {
            self.user_id = Some(u.id);
            self.username = Some(u.username.clone());
        }
        self.ip = identity.ip;
        self
    }

    pub fn target(mut self, target: impl Into<String>) -> Self {
        self.target = Some(target.into());
        self
    }

    pub fn workspace(mut self, ws: &str) -> Self {
        self.workspace = Some(ws.to_owned());
        self
    }

    pub fn ip(mut self, ip: ClientIp) -> Self {
        self.ip = ip.0;
        self
    }
}

impl Store {
    /// Records an event (never fails the caller's action: errors are logged).
    pub async fn audit(&self, e: Audit) {
        let action = e.action;
        let result = self
            .with(move |c| {
                c.execute(
                    "INSERT INTO audit (at, user_id, username, workspace, action, target, ip) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![now_ms(), e.user_id, e.username, e.workspace, e.action, e.target, e.ip.map(|i| i.to_string())],
                )
                .map(drop)
                .map_err(db_err)
            })
            .await;
        if let Err(err) = result {
            tracing::warn!(action, "cannot write the audit log: {err}");
        }
    }

    /// Events `viewer` may see, newest first: their own and those of workspaces they own.
    /// `None` (no accounts, or the command line): every event.
    pub async fn audit_log(
        &self,
        viewer: Owner,
        before: Option<i64>,
        limit: usize,
    ) -> Result<Vec<AuditEvent>> {
        self.with(move |c| {
            let mut stmt = c
                .prepare(
                    "SELECT id, at, username, workspace, action, target, ip FROM audit
                     WHERE (?1 IS NULL OR user_id = ?1 OR workspace IN
                            (SELECT workspace_id FROM members WHERE user_id = ?1 AND role = 'owner'))
                       AND (?2 IS NULL OR id < ?2)
                     ORDER BY id DESC LIMIT ?3",
                )
                .map_err(db_err)?;
            let rows = stmt
                .query_map(params![viewer, before, limit.clamp(1, 1000) as i64], |r| {
                    Ok(AuditEvent {
                        id: r.get(0)?,
                        at: r.get(1)?,
                        username: r.get(2)?,
                        workspace: r.get(3)?,
                        action: r.get(4)?,
                        target: r.get(5)?,
                        ip: r.get(6)?,
                    })
                })
                .map_err(db_err)?;
            rows.collect::<std::result::Result<Vec<_>, _>>()
                .map_err(db_err)
        })
        .await
    }

    /// Deletes audit events older than [`AUDIT_DAYS`].
    pub async fn prune_audit(&self) -> Result<usize> {
        self.with(|c| {
            let cutoff = now_ms() - i64::try_from(AUDIT_DAYS * 24 * 3600 * 1000).unwrap_or(0);
            c.execute("DELETE FROM audit WHERE at < ?1", [cutoff])
                .map_err(db_err)
        })
        .await
    }
}

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(routes!(list_audit))
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
struct AuditParams {
    /// Only events with an id below this (paging).
    before: Option<i64>,
    limit: Option<usize>,
}

#[derive(Debug, utoipa::ToSchema)]
pub struct AuditList {
    pub events: Vec<AuditEvent>,
}

#[utoipa::path(get, path = "/audit", tag = "auth", params(AuditParams), responses((status = 200, description = "The caller's events and those of the workspaces they own, newest first", body = AuditList)))]
async fn list_audit(
    State(state): Shared,
    identity: Identity,
    Query(p): Query<AuditParams>,
) -> ApiResult<Json<Value>> {
    let events = state
        .store
        .audit_log(identity.owner(), p.before, p.limit.unwrap_or(50))
        .await?;
    Ok(Json(json!({ "events": events })))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn limiter_blocks_after_max_failures() {
        let l = Limiter::new(3, Duration::from_secs(60));
        for _ in 0..2 {
            l.fail("ana");
        }
        assert!(l.blocked("ana").is_none());
        l.fail("ana");
        assert!(
            l.blocked("ana")
                .is_some_and(|d| d <= Duration::from_secs(60))
        );
        assert!(l.blocked("bo").is_none());
        l.reset("ana");
        assert!(l.blocked("ana").is_none());
        let short = Limiter::new(1, Duration::from_millis(1));
        short.fail("x");
        std::thread::sleep(Duration::from_millis(5));
        assert!(short.blocked("x").is_none(), "the window ended");
    }

    #[test]
    fn hosts() {
        let none: &[String] = &[];
        for ok in [
            "localhost:7878",
            "127.0.0.1:7878",
            "[::1]:7878",
            "app.localhost",
            "127.0.0.2",
            "LOCALHOST.",
        ] {
            assert!(host_allowed(Some(ok), none), "{ok}");
        }
        for bad in [
            "evil.example:7878",
            "192.168.1.5:7878",
            "[2001:db8::1]:80",
            "localhost.evil.com",
        ] {
            assert!(!host_allowed(Some(bad), none), "{bad}");
        }
        assert!(host_allowed(Some("devbox:7878"), &["devbox".to_owned()]));
        assert!(host_allowed(None, none));
    }
}
