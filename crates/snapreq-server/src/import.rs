//! `POST /api/v1/collections/import` (PLAN P9.1): a file from another tool becomes a new
//! collection in the workspace. Credentials found in it are sealed into `.secrets`.

use std::sync::Arc;

use axum::Json;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use serde::Deserialize;
use serde_json::{Value, json};
use snapreq_interop::{Format, Options};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AppState;
use crate::api::{ApiError, ApiResult, ErrorBody, JsonDocument, collection_dir};
use crate::auth::Identity;
use crate::security::Audit;
use crate::workspaces::EditSpace;

/// Imports may be large (OpenAPI documents, HAR recordings); other routes keep axum's 2 MiB.
const BODY_LIMIT: usize = 32 << 20;

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(import_collection))
        .layer(DefaultBodyLimit::max(BODY_LIMIT))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct ImportBody {
    /// The file's text.
    content: String,
    /// `curl`, `postman`, `openapi`, `insomnia`, `har` or `hurl`; detected when absent.
    #[serde(default)]
    format: Option<String>,
    /// Id of the new collection; default: derived from its name.
    #[serde(default)]
    id: Option<String>,
    /// Collection name; default: from the file.
    #[serde(default)]
    name: Option<String>,
    /// Postman environment files to add as environments.
    #[serde(default)]
    environments: Vec<String>,
}

#[utoipa::path(post, path = "/collections/import", tag = "collections", request_body = ImportBody, responses((status = 201, description = "Imported: `id`, `format`, counts, `environments`, `secrets` and `warnings`", body = JsonDocument), (status = 400, description = "Not a supported format, or invalid", body = ErrorBody), (status = 403, description = "Viewers cannot import", body = ErrorBody), (status = 409, description = "The id is taken", body = ErrorBody)))]
async fn import_collection(
    State(state): State<Arc<AppState>>,
    identity: Identity,
    space: EditSpace,
    Json(body): Json<ImportBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let format = body
        .format
        .as_deref()
        .filter(|f| !f.is_empty())
        .map(str::parse::<Format>)
        .transpose()
        .map_err(ApiError::bad_request)?;
    let key = state.secret_key.clone();
    let target_space = space.0.clone();
    let requested_id = body.id.clone().filter(|i| !i.is_empty());
    let result = tokio::task::spawn_blocking(move || {
        let format = match format {
            Some(f) => f,
            None => snapreq_interop::detect(&body.content)
                .map_err(|e| ApiError::bad_request(e.to_string()))?,
        };
        let mut imported = snapreq_interop::import(
            &body.content,
            &Options {
                format: Some(format),
                name: body.name.clone().filter(|n| !n.is_empty()),
            },
        )
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
        for env in &body.environments {
            snapreq_interop::add_postman_environment(&mut imported, env)
                .map_err(|e| ApiError::bad_request(format!("environment: {e}")))?;
        }
        let id = requested_id
            .unwrap_or_else(|| snapreq_interop::slug(&imported.collection.name, "imported"));
        let dir = collection_dir(&target_space, &id)?;
        if dir.exists() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "conflict",
                format!("`{id}` already exists"),
            ));
        }
        let summary = imported
            .write(&dir, Some(&key))
            .map_err(|e| ApiError::internal(e.to_string()))?;
        Ok((id, format, summary))
    })
    .await
    .map_err(|e| ApiError::internal(e.to_string()))?;
    let (id, format, summary) = result?;
    state
        .store
        .audit(
            Audit::new("collection.imported")
                .by(&identity)
                .workspace(&space.id)
                .target(format!("{id} ({format})")),
        )
        .await;
    let mut doc = serde_json::to_value(&summary).unwrap_or_else(|_| json!({}));
    doc["id"] = id.into();
    doc["format"] = format.as_str().into();
    Ok((StatusCode::CREATED, Json(doc)))
}
