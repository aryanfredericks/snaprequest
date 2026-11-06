//! Requests as code and history as HAR (PLAN P9.2).
//!
//! `POST /api/v1/requests/export` builds a request exactly as `/requests/execute` would, without
//! sending it, and returns it as a cURL command or a code snippet: secret values become
//! environment variable references, never values. `GET /api/v1/history/{id}/har` returns a stored
//! exchange as HAR (history is already redacted).

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, State};
use axum::http::header;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use snapreq_collection::{Environment, RequestFile, Variables};
use snapreq_core::Request;
use snapreq_interop::export::{Target, snippet};
use snapreq_test::Session;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AppState;
use crate::api::{
    ApiError, ApiResult, ErrorBody, JsonDocument, key_path, load_collection, load_secrets,
    viewer_overrides,
};
use crate::auth::Identity;
use crate::workspaces::{Role, Space};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(export_request))
        .routes(routes!(history_har))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct ExportBody {
    /// `curl`, `python`, `javascript`, `go` or `rust`.
    format: String,
    /// A complete request (the core JSON model).
    #[schema(value_type = Option<Object>)]
    request: Option<Request>,
    /// Or a collection request: collection id and request key (as for `/requests/execute`).
    collection: Option<String>,
    key: Option<String>,
    /// Unsaved contents of the request file.
    #[schema(value_type = Option<Object>)]
    file: Option<RequestFile>,
    env: Option<String>,
    #[serde(default)]
    vars: BTreeMap<String, String>,
}

#[utoipa::path(post, path = "/requests/export", tag = "requests", request_body = ExportBody, responses((status = 200, description = "`{ format, content }`: the request as code; secret values are read from `SNAPREQ_SECRET_<NAME>` environment variables", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not build", body = ErrorBody)))]
async fn export_request(
    State(state): State<Arc<AppState>>,
    identity: Identity,
    headers: axum::http::HeaderMap,
    Json(body): Json<ExportBody>,
) -> ApiResult<Json<Value>> {
    let target: Target = body.format.parse().map_err(ApiError::bad_request)?;
    let (request, named) = build(
        &state,
        &identity,
        &headers,
        Built {
            request: body.request,
            collection: body.collection,
            key: body.key,
            file: body.file,
            env: body.env,
            vars: body.vars,
        },
    )
    .await?;
    let content = snippet(&request, target, &named);
    Ok(Json(
        json!({ "format": target.as_str(), "content": content }),
    ))
}

/// What names the request to build: a core request, or a collection request (with unsaved
/// edits, environment and variables, as for `/requests/execute`).
#[derive(Debug)]
pub(crate) struct Built {
    pub request: Option<Request>,
    pub collection: Option<String>,
    pub key: Option<String>,
    pub file: Option<RequestFile>,
    pub env: Option<String>,
    pub vars: BTreeMap<String, String>,
}

/// Builds a request exactly as `/requests/execute` would (without scripts), with the names and
/// values of its secret variables.
pub(crate) async fn build(
    state: &AppState,
    identity: &Identity,
    headers: &axum::http::HeaderMap,
    body: Built,
) -> ApiResult<(Request, Vec<(String, String)>)> {
    let (request, named) = match (body.request, body.collection, body.key) {
        (Some(req), None, None) => {
            if body.env.is_some() || !body.vars.is_empty() || body.file.is_some() {
                return Err(ApiError::bad_request(
                    "`env`, `vars` and `file` only apply to collection requests",
                ));
            }
            (req, Vec::new())
        }
        (None, Some(cid), Some(key)) => {
            let ws = crate::workspaces::requested(headers);
            let space = Space::resolve(state, identity, &ws).await?;
            if body.file.is_some() || !body.vars.is_empty() {
                space
                    .require(Role::Editor)
                    .map_err(|_| viewer_overrides(&space))?;
            }
            let mut collection = load_collection(&space, &cid)?;
            if let Some(file) = body.file {
                file.validate().map_err(ApiError::invalid)?;
                key_path(&key, ".toml")?;
                collection
                    .put_request(&key, file)
                    .map_err(ApiError::not_found)?;
            }
            if collection.find(&key).is_none() {
                return Err(ApiError::not_found(format!(
                    "no request `{key}` in collection `{cid}`"
                )));
            }
            let environment = body
                .env
                .as_deref()
                .map(|e| Environment::load(&collection, e))
                .transpose()
                .map_err(|e| ApiError::invalid(e.to_string()))?;
            let secrets = load_secrets(state, &collection, body.env.as_deref())
                .map_err(|e| ApiError::invalid(e.to_string()))?;
            let mut session = Session::new(
                &collection,
                environment.as_ref(),
                &secrets,
                state.client.clone(),
            );
            let mut overrides = Variables::default();
            for (k, v) in &body.vars {
                overrides.set(k, v);
            }
            session.overrides = overrides;
            let scopes = session.scopes(&key);
            let request = snapreq_collection::build_request(&collection, &key, &scopes)
                .map_err(|e| ApiError::invalid(e.to_string()))?;
            (request, named_secrets(&scopes))
        }
        _ => {
            return Err(ApiError::bad_request(
                "give either `request`, or `collection` and `key`",
            ));
        }
    };
    Ok((request, named))
}

/// Secret variables (declared secret anywhere, even when a higher scope sets them) with values.
pub(crate) fn named_secrets(scopes: &snapreq_collection::Scopes) -> Vec<(String, String)> {
    scopes
        .visible()
        .into_iter()
        .filter(|(v, _)| v.secret || scopes.is_declared_secret(&v.name))
        .filter_map(|(v, _)| Some((v.name.clone(), v.value.clone()?)))
        .collect()
}

#[utoipa::path(get, path = "/history/{id}/har", tag = "history", params(("id" = i64, Path, description = "History entry id")), responses((status = 200, description = "The exchange as an HTTP Archive (HAR 1.2), as a download", body = JsonDocument), (status = 404, description = "Not found", body = ErrorBody)))]
async fn history_har(
    State(state): State<Arc<AppState>>,
    identity: Identity,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Response> {
    let entry = state
        .store
        .history_entry(identity.owner(), id)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("no history entry {id}")))?;
    let doc = snapreq_interop::har_export::from_history(&[(
        entry.summary.created_at,
        &entry.request,
        entry.response.as_ref(),
    )]);
    let text = serde_json::to_string_pretty(&doc).unwrap_or_default();
    Ok((
        [
            (header::CONTENT_TYPE, "application/json".to_owned()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"snapreq-{id}.har\""),
            ),
        ],
        text,
    )
        .into_response())
}
