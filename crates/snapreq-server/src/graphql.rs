//! `POST /api/v1/requests/graphql-schema`: a GraphQL schema by introspection (PLAN P10.3).
//!
//! The request is built as `/requests/export` builds it (collection requests with inheritance,
//! environment and variables), its body replaced by the introspection query, and sent. Viewers may
//! introspect saved requests: only the body changes, never where the request goes. Nothing is
//! stored in history.

use std::collections::BTreeMap;
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use serde_json::{Value, json};
use snapreq_collection::RequestFile;
use snapreq_core::{Body, Request};
use snapreq_interop::graphql;
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AppState;
use crate::api::{ApiError, ApiResult, ErrorBody, JsonDocument};
use crate::auth::Identity;
use crate::export::{Built, build};

pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new().routes(routes!(graphql_schema))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct SchemaBody {
    /// A complete request (the core JSON model): its URL, headers, auth and settings are used.
    #[schema(value_type = Option<Object>)]
    request: Option<Request>,
    /// Or a collection request: collection id and request key (as for `/requests/execute`).
    collection: Option<String>,
    key: Option<String>,
    /// Unsaved contents of the request file (editors).
    #[schema(value_type = Option<Object>)]
    file: Option<RequestFile>,
    env: Option<String>,
    #[serde(default)]
    vars: BTreeMap<String, String>,
}

#[utoipa::path(post, path = "/requests/graphql-schema", tag = "requests", request_body = SchemaBody, responses((status = 200, description = "`{ schema, sdl }`: the `__schema` object of the introspection answer and the schema as SDL", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "The request does not build, cannot be sent, or the answer has no schema", body = ErrorBody)))]
async fn graphql_schema(
    State(state): State<Arc<AppState>>,
    identity: Identity,
    headers: axum::http::HeaderMap,
    Json(body): Json<SchemaBody>,
) -> ApiResult<Json<Value>> {
    let (mut request, _) = build(
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
    if request.method == axum::http::Method::GET {
        request.method = axum::http::Method::POST;
    }
    request.body = Body::Graphql {
        query: graphql::INTROSPECTION_QUERY.into(),
        variables: None,
        operation_name: Some("IntrospectionQuery".into()),
    };
    request
        .effective_url()
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    let secrets = request.secrets();
    let response = state
        .client
        .execute(&request)
        .await
        .map_err(|e| ApiError::invalid(snapreq_test::report::hide(&e.to_string(), &secrets)))?;
    let answer: Value = serde_json::from_slice(&response.body).map_err(|_| {
        ApiError::invalid(format!(
            "the server answered {} with a body that is not JSON (is this a GraphQL endpoint?)",
            response.status
        ))
    })?;
    let schema = graphql::schema_of(&answer).map_err(|e| ApiError::invalid(e.to_string()))?;
    Ok(Json(
        json!({ "schema": schema, "sdl": graphql::to_sdl(schema) }),
    ))
}
