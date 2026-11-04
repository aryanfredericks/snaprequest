//! The REST API under `/api/v1` (see `docs/SERVER.md`).
//!
//! Collections live in the workspace directory, one subdirectory each (the directory name
//! is the collection id). Every write goes through the collection file types, so files
//! stay in the normal on-disk format and can be edited by hand or with the CLI.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::{Path as UrlPath, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Deserialize;
use serde_json::{Value, json};
use snapreq_collection::load::COLLECTION_FILE;
use snapreq_collection::save::{save_collection, save_environment, save_flow, save_request};
use snapreq_collection::{
    Collection, CollectionFile, Environment, FlowFile, Item, RequestFile, Secrets, Variables,
};
use snapreq_core::Request;
use snapreq_test::report::{captures_json, scrub_outcomes, secrets_of};
use snapreq_test::{Plan, Report, RunOptions, RunOutcome, Session, execute, resolve_unit};
use utoipa_axum::router::OpenApiRouter;
use utoipa_axum::routes;

use crate::AppState;
use crate::auth::Identity;
use crate::store::{HistoryEntry, HistoryQuery, Owner, RunEntry};
use crate::workspaces::{EditSpace, Role, Space};

pub(crate) type Shared = State<Arc<AppState>>;

/// An error as JSON: `{ "error": { "kind", "message" } }`.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub kind: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn new(status: StatusCode, kind: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            kind,
            message: message.into(),
        }
    }

    pub(crate) fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, "bad_request", message)
    }

    pub(crate) fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", message)
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Self::new(StatusCode::UNPROCESSABLE_ENTITY, "invalid", message)
    }

    pub(crate) fn internal(message: impl Into<String>) -> Self {
        Self::new(StatusCode::INTERNAL_SERVER_ERROR, "internal", message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(json!({ "error": { "kind": self.kind, "message": self.message } })),
        )
            .into_response()
    }
}

impl From<crate::Error> for ApiError {
    fn from(e: crate::Error) -> Self {
        match e {
            crate::Error::Account(message) => Self::invalid(message),
            other => Self::internal(other.to_string()),
        }
    }
}

pub(crate) type ApiResult<T> = Result<T, ApiError>;

/// The `/api/v1` routes with their OpenAPI description.
pub fn router() -> OpenApiRouter<Arc<AppState>> {
    OpenApiRouter::new()
        .routes(routes!(execute_request))
        .routes(routes!(list_history, clear_history))
        .routes(routes!(get_history, delete_history))
        .routes(routes!(list_runs, start_run))
        .routes(routes!(get_run))
        .routes(routes!(get_run_report))
        .routes(routes!(list_collections, create_collection))
        .routes(routes!(get_collection, put_collection, delete_collection))
        .routes(routes!(get_request, put_request, delete_request))
        .routes(routes!(get_flow, put_flow, delete_flow))
        .routes(routes!(
            get_environment,
            put_environment,
            delete_environment
        ))
        .routes(routes!(list_variables))
        .merge(crate::tree::router())
}

/// `{ "error": { "kind", "message" } }`
#[derive(Debug, utoipa::ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, utoipa::ToSchema)]
pub struct ErrorDetail {
    /// `bad_request`, `unauthorized`, `forbidden`, `not_found`, `conflict`, `invalid` or
    /// `internal`.
    pub kind: String,
    pub message: String,
}

/// A JSON document; its shape is described in `docs/SERVER.md`.
#[derive(Debug, utoipa::ToSchema)]
#[schema(value_type = Object)]
pub struct JsonDocument(pub Value);

#[derive(Debug, utoipa::ToSchema)]
pub struct HistoryList {
    pub entries: Vec<crate::store::HistorySummary>,
}

#[derive(Debug, utoipa::ToSchema)]
pub struct RunList {
    pub runs: Vec<crate::store::RunSummary>,
}

// --- Names and paths -------------------------------------------------------------

/// One path segment: letters, digits, `_`, `-`, `.`; not starting with `.`.
pub(crate) fn valid_segment(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('.')
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The directory of collection `id` in the workspace, which must be a plain name.
pub(crate) fn collection_dir(space: &Space, id: &str) -> ApiResult<PathBuf> {
    if !valid_segment(id) {
        return Err(ApiError::bad_request(format!(
            "invalid collection id `{id}` (letters, digits, `_`, `-`, `.`)"
        )));
    }
    Ok(space.dir.join(id))
}

pub(crate) fn load_collection(space: &Space, id: &str) -> ApiResult<Collection> {
    let dir = collection_dir(space, id)?;
    if !dir.join(COLLECTION_FILE).is_file() {
        return Err(ApiError::not_found(format!("no collection `{id}`")));
    }
    Collection::load(&dir)
        .map(|(c, _)| c)
        .map_err(|e| ApiError::invalid(e.to_string()))
}

/// A request or flow key (`users/create-user`) as a relative path, or an error. Keys must
/// not leave the collection or collide with collection files.
pub(crate) fn key_path(key: &str, suffix: &str) -> ApiResult<PathBuf> {
    let key = key.trim_matches('/');
    let parts: Vec<&str> = key.split('/').collect();
    if key.is_empty() || !parts.iter().all(|p| valid_segment(p)) {
        return Err(ApiError::bad_request(format!(
            "invalid key `{key}` (segments of letters, digits, `_`, `-`, `.`)"
        )));
    }
    let last = parts[parts.len() - 1];
    if parts[0] == "environments"
        || last == "snapreq"
        || last == "folder"
        || last.ends_with(".flow")
    {
        return Err(ApiError::bad_request(format!("`{key}` is a reserved name")));
    }
    let mut path: PathBuf = parts.iter().collect();
    path.set_file_name(format!("{last}{suffix}"));
    Ok(path)
}

/// Viewers send saved requests only: unsaved edits, variables and data rows could point the
/// workspace's secrets at another host.
fn viewer_overrides(space: &Space) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "forbidden",
        format!(
            "viewers of workspace `{}` send saved requests only, without unsaved edits, variables or data",
            space.id
        ),
    )
}

/// The secret values of a collection, sealed ones opened with the server's key.
pub(crate) fn load_secrets(
    state: &AppState,
    c: &Collection,
    env: Option<&str>,
) -> snapreq_collection::Result<Secrets> {
    Secrets::load_with_key(c, env, std::env::vars(), Some(&state.secret_key))
}

pub(crate) fn remove_file(path: &Path, what: &str) -> ApiResult<StatusCode> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(StatusCode::NO_CONTENT),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            Err(ApiError::not_found(format!("no {what}")))
        }
        Err(e) => Err(ApiError::internal(e.to_string())),
    }
}

// --- Executing ---------------------------------------------------------------

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct ExecuteBody {
    /// A complete request (the core JSON model).
    #[schema(value_type = Option<Object>)]
    pub request: Option<Request>,
    /// Or a collection request: collection id and request key.
    pub collection: Option<String>,
    pub key: Option<String>,
    /// With `collection` and `key`: unsaved contents of the request file (as JSON), sent
    /// instead of the saved file, with the same inheritance and variables. Not written.
    #[schema(value_type = Option<Object>)]
    pub file: Option<RequestFile>,
    pub env: Option<String>,
    /// Variables for this execution (highest precedence).
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    /// Record the raw bytes sent and received (a `wire` trace event), whatever the
    /// request's own settings say.
    #[serde(default)]
    pub capture_wire: bool,
}

fn vars_of(map: &BTreeMap<String, String>) -> Variables {
    let mut v = Variables::default();
    for (k, val) in map {
        v.set(k, val);
    }
    v
}

/// The server's view of one sent request.
fn exchange_json(history_id: Option<i64>, run: &snapreq_test::RequestRun) -> Value {
    let secrets = secrets_of(run);
    let mut assertions = run.assertions.clone();
    scrub_outcomes(&mut assertions, &secrets);
    let (response, error) = match &run.outcome {
        RunOutcome::Response(r) => {
            let mut doc = json!(r);
            // The final URL can carry secret values (`?key={{token}}`); the trace hides them too.
            doc["url"] = json!(snapreq_test::report::hide(&r.url, &secrets));
            (doc, Value::Null)
        }
        RunOutcome::Invalid(why) => (
            Value::Null,
            json!({ "kind": "invalid_request", "message": why }),
        ),
        RunOutcome::Failed(e) => (Value::Null, error_json(e)),
    };
    json!({
        "history_id": history_id,
        "response": response,
        "error": error,
        "assertions": assertions,
        "captures": captures_json(&run.captures, &secrets),
    })
}

fn error_json(e: &snapreq_core::Error) -> Value {
    match e {
        snapreq_core::Error::Execution(x) => json!({
            "kind": "execution",
            "message": x.message,
            "phase": x.phase,
            "timed_out": x.timed_out,
            "trace": x.trace,
        }),
        other => json!({ "kind": "invalid_request", "message": other.to_string() }),
    }
}

#[utoipa::path(post, path = "/requests/execute", tag = "requests", request_body = ExecuteBody, responses((status = 200, description = "The exchange (`response` or `error`), stored in history", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
/// `POST /api/v1/requests/execute`
async fn execute_request(
    State(state): Shared,
    identity: Identity,
    headers: axum::http::HeaderMap,
    Json(body): Json<ExecuteBody>,
) -> ApiResult<Json<Value>> {
    let ws = crate::workspaces::requested(&headers);
    Ok(Json(
        execute_body(&state, &identity, &ws, body, None).await?,
    ))
}

/// Executes an [`ExecuteBody`] (also used by the WebSocket endpoint, which passes a
/// trace sink for live events).
/// Collection requests come from workspace `ws`, which the caller must belong to.
pub(crate) async fn execute_body(
    state: &AppState,
    identity: &Identity,
    ws: &str,
    body: ExecuteBody,
    trace: Option<snapreq_core::client::TraceSink>,
) -> ApiResult<Value> {
    let owner = identity.owner();
    match (body.request, body.collection, body.key) {
        (Some(mut req), None, None) => {
            if body.env.is_some() || !body.vars.is_empty() || body.file.is_some() {
                return Err(ApiError::bad_request(
                    "`env`, `vars` and `file` only apply to collection requests",
                ));
            }
            req.settings.capture_wire |= body.capture_wire;
            req.effective_url()
                .map_err(|e| ApiError::invalid(e.to_string()))?;
            let result = state.client.execute_with(&req, trace).await;
            let id = state
                .store
                .add_history(
                    owner,
                    HistoryEntry::from_exchange(&req, result.as_ref(), None),
                )
                .await?;
            Ok(match result {
                Ok(resp) => json!({ "history_id": id, "response": resp, "error": null }),
                Err(e) => json!({ "history_id": id, "response": null, "error": error_json(&e) }),
            })
        }
        (None, Some(cid), Some(key)) => {
            let space = Space::resolve(state, identity, ws).await?;
            if body.file.is_some() || !body.vars.is_empty() {
                // Edits and overrides could send the workspace's secrets anywhere.
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
            session.overrides = vars_of(&body.vars);
            hide_declared(&mut session, &key);
            session.trace = trace;
            let wire = body.capture_wire;
            let run = session
                .send(&key, |r| r.settings.capture_wire |= wire)
                .await;
            if let RunOutcome::Invalid(why) = &run.outcome {
                return Err(ApiError::invalid(why.clone()));
            }
            let req = run
                .request
                .as_ref()
                .ok_or_else(|| ApiError::internal("no request"))?;
            let outcome = match &run.outcome {
                RunOutcome::Response(r) => Ok(&**r),
                RunOutcome::Failed(e) => Err(e),
                RunOutcome::Invalid(_) => unreachable!("handled above"),
            };
            let mut entry = HistoryEntry::from_exchange(req, outcome, Some((&cid, &key)));
            entry.workspace = Some(space.id.clone());
            // Values captured as secret by this request are hidden in its own history too.
            let captured: Vec<String> = run
                .captures
                .iter()
                .filter(|c| c.secret)
                .filter_map(|c| c.value.clone())
                .collect();
            if !captured.is_empty() {
                entry.request = snapreq_test::report::hide_json(entry.request, &captured);
                entry.response = entry
                    .response
                    .map(|r| snapreq_test::report::hide_json(r, &captured));
            }
            let id = state.store.add_history(owner, entry).await?;
            Ok(exchange_json(Some(id), &run))
        }
        _ => Err(ApiError::bad_request(
            "give either `request`, or `collection` and `key`",
        )),
    }
}

/// `--var`-style values of variables declared secret are hidden in every request.
fn hide_declared(session: &mut Session<'_>, key: &str) {
    let scopes = session.scopes(key);
    let values: Vec<String> = session
        .overrides
        .0
        .iter()
        .filter(|v| scopes.is_declared_secret(&v.name))
        .filter_map(|v| v.value.clone())
        .collect();
    for v in values {
        session.hide(v);
    }
}

// --- History -----------------------------------------------------------------

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
struct HistoryParams {
    /// Substring of the URL (case-insensitive).
    url: Option<String>,
    method: Option<String>,
    status: Option<u16>,
    /// Status class, `1` to `5` (`4` = 400–499).
    class: Option<u16>,
    /// `true`: only failures without a response; `false`: only responses.
    failed: Option<bool>,
    /// Only requests of this collection (of the workspace in `X-SnapReq-Workspace`).
    collection: Option<String>,
    before: Option<i64>,
    limit: Option<usize>,
}

#[utoipa::path(get, path = "/history", tag = "history", params(HistoryParams), responses((status = 200, description = "History summaries, newest first", body = HistoryList)))]
/// `GET /api/v1/history?url=&method=&status=&class=&failed=&collection=&before=&limit=`
async fn list_history(
    State(state): Shared,
    identity: Identity,
    headers: axum::http::HeaderMap,
    Query(p): Query<HistoryParams>,
) -> ApiResult<Json<Value>> {
    let entries = state
        .store
        .history(
            identity.owner(),
            HistoryQuery {
                url_contains: p.url,
                method: p.method,
                status: p.status,
                status_class: p.class.filter(|c| (1..=5).contains(c)),
                failed: p.failed,
                workspace: p
                    .collection
                    .is_some()
                    .then(|| crate::workspaces::requested(&headers)),
                collection: p.collection,
                before_id: p.before,
                limit: p.limit.unwrap_or(50),
            },
        )
        .await?;
    Ok(Json(json!({ "entries": entries })))
}

#[utoipa::path(get, path = "/history/{id}", tag = "history", params(("id" = i64, Path, description = "History entry id")), responses((status = 200, description = "Summary with stored request and response", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn get_history(
    State(state): Shared,
    identity: Identity,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<Value>> {
    match state.store.history_entry(identity.owner(), id).await? {
        Some(e) => Ok(Json(json!(e))),
        None => Err(ApiError::not_found(format!("no history entry {id}"))),
    }
}

#[utoipa::path(delete, path = "/history/{id}", tag = "history", params(("id" = i64, Path, description = "History entry id")), responses((status = 204, description = "Deleted"), (status = 404, description = "Not found", body = ErrorBody)))]
async fn delete_history(
    State(state): Shared,
    identity: Identity,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<StatusCode> {
    match state
        .store
        .delete_history(identity.owner(), Some(id))
        .await?
    {
        0 => Err(ApiError::not_found(format!("no history entry {id}"))),
        _ => Ok(StatusCode::NO_CONTENT),
    }
}

#[utoipa::path(delete, path = "/history", tag = "history", responses((status = 200, description = "`{ deleted }`: every entry is removed", body = JsonDocument)))]
async fn clear_history(State(state): Shared, identity: Identity) -> ApiResult<Json<Value>> {
    let deleted = state.store.delete_history(identity.owner(), None).await?;
    Ok(Json(json!({ "deleted": deleted })))
}

// --- Runs --------------------------------------------------------------------

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
pub struct RunBody {
    pub collection: String,
    /// A flow or folder key; absent runs every request of the collection.
    pub target: Option<String>,
    pub env: Option<String>,
    #[serde(default)]
    pub vars: BTreeMap<String, String>,
    /// One iteration per row.
    #[serde(default)]
    #[schema(value_type = Vec<Object>)]
    pub data: Vec<BTreeMap<String, Value>>,
    pub iterations: Option<usize>,
    #[serde(default)]
    pub bail: bool,
    pub delay_ms: Option<u64>,
    pub parallel: Option<usize>,
    /// Record the raw bytes of every exchange (`wire` trace events in the stored report).
    #[serde(default)]
    pub capture_wire: bool,
    /// Skip TLS certificate verification for every request (`snapreq run -k`).
    #[serde(default)]
    pub insecure: bool,
    /// Time limit per request in milliseconds, `0` for none (`snapreq run -t`).
    pub timeout_ms: Option<u64>,
    /// Flows only: the first step to run (1-based). With `resume`, the captured values of the
    /// earlier steps come from that run.
    pub from_step: Option<usize>,
    /// Flows only: the last step to run.
    pub to_step: Option<usize>,
    /// Flows only: the id of an earlier run of this flow whose state before `from_step` to
    /// start from (kept in memory for an hour; never stored or returned).
    pub resume: Option<i64>,
}

#[utoipa::path(post, path = "/runs", tag = "runs", request_body = RunBody, responses((status = 200, description = "The run document plus its stored `id`", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
/// `POST /api/v1/runs`: runs to completion, stores the result and returns its summary.
async fn start_run(
    State(state): Shared,
    space: Space,
    identity: Identity,
    Json(body): Json<RunBody>,
) -> ApiResult<Json<Value>> {
    Ok(Json(
        run_body(&state, &space, identity.owner(), body, None).await?,
    ))
}

/// Runs a [`RunBody`]; `live` receives progress and trace events (WebSocket).
pub(crate) async fn run_body(
    state: &AppState,
    space: &Space,
    owner: Owner,
    body: RunBody,
    live: Option<snapreq_test::session::Live>,
) -> ApiResult<Value> {
    if !body.vars.is_empty() || !body.data.is_empty() {
        space
            .require(Role::Editor)
            .map_err(|_| viewer_overrides(space))?;
    }
    let collection = load_collection(space, &body.collection)?;
    // Flow states are kept per workspace and collection.
    let state_key = format!("{}/{}", space.id, body.collection);
    let unit = resolve_unit(&collection, body.target.as_deref()).map_err(ApiError::not_found)?;
    let environment = body
        .env
        .as_deref()
        .map(|e| Environment::load(&collection, e))
        .transpose()
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    let secrets = load_secrets(state, &collection, body.env.as_deref())
        .map_err(|e| ApiError::invalid(e.to_string()))?;
    if !body.data.is_empty() && body.iterations.is_some() {
        return Err(ApiError::bad_request("give either `data` or `iterations`"));
    }
    if body.iterations == Some(0) || body.parallel == Some(0) {
        return Err(ApiError::bad_request(
            "`iterations` and `parallel` must be at least 1",
        ));
    }
    let rows: Vec<Variables> = body
        .data
        .iter()
        .map(|row| {
            let mut v = Variables::default();
            for (k, val) in row {
                v.set(
                    k,
                    match val {
                        Value::String(s) => s.clone(),
                        other => other.to_string(),
                    },
                );
            }
            v
        })
        .collect();
    let stepped = body.from_step.is_some() || body.to_step.is_some() || body.resume.is_some();
    let mut flow_steps = snapreq_test::FlowSteps::default();
    if stepped {
        let snapreq_test::Unit::Flow(flow) = &unit else {
            return Err(ApiError::bad_request(
                "`from_step`, `to_step` and `resume` apply to flows only",
            ));
        };
        if !body.data.is_empty() || body.iterations.is_some_and(|n| n > 1) {
            return Err(ApiError::bad_request(
                "a flow run by steps has a single iteration",
            ));
        }
        let total = flow.file.steps.len();
        let from = body.from_step.unwrap_or(1);
        let to = body.to_step.unwrap_or(total);
        if from == 0 || from > total || to < from || to > total {
            return Err(ApiError::bad_request(format!(
                "steps {from}–{to} are not in this flow (1–{total})"
            )));
        }
        let start = match body.resume {
            Some(run) => Some(
                state
                    .flow_states
                    .get(owner, run, &state_key, &flow.key, from)
                    .ok_or_else(|| {
                        ApiError::new(
                            StatusCode::CONFLICT,
                            "conflict",
                            format!(
                                "the state before step {from} of run {run} is no longer available; run the flow from the start"
                            ),
                        )
                    })?,
            ),
            None if from > 1 => {
                return Err(ApiError::bad_request(
                    "starting after step 1 needs `resume` (a run whose captured values to use)",
                ));
            }
            None => None,
        };
        flow_steps = snapreq_test::FlowSteps {
            from,
            to: Some(to),
            start,
        };
    }
    let overrides = vars_of(&body.vars);
    let scope_key = match &unit {
        snapreq_test::Unit::Flow(f) => f.key.clone(),
        snapreq_test::Unit::Requests(keys) => keys[0].clone(),
    };
    let declared = snapreq_collection::Scopes::for_request(
        &collection,
        &scope_key,
        environment.as_ref(),
        &Variables::default(),
        &Variables::default(),
    );
    let secret_vars: Vec<String> = overrides
        .0
        .iter()
        .chain(rows.iter().flat_map(|r| &r.0))
        .map(|v| v.name.clone())
        .filter(|n| declared.is_declared_secret(n))
        .collect();
    let hidden: Vec<String> = overrides
        .0
        .iter()
        .chain(rows.iter().flat_map(|r| &r.0))
        .filter(|v| declared.is_declared_secret(&v.name))
        .filter_map(|v| v.value.clone())
        .collect();
    let plan = Plan {
        collection: &collection,
        environment: environment.as_ref(),
        secrets: &secrets,
        client: state.client.clone(),
        overrides,
        hidden,
        rows,
        iterations: body.iterations.unwrap_or(1),
        options: RunOptions {
            bail: body.bail,
            delay: body.delay_ms.map(std::time::Duration::from_millis),
            parallel: body.parallel.unwrap_or(1),
        },
        live,
        flow_steps,
    };
    let planned = if plan.rows.is_empty() {
        plan.iterations
    } else {
        plan.rows.len()
    };
    let (wire, insecure, timeout) = (body.capture_wire, body.insecure, body.timeout_ms);
    let iterations = execute(&plan, &unit, |r| {
        r.settings.capture_wire |= wire;
        if insecure {
            r.settings.tls.verify = false;
        }
        if let Some(ms) = timeout {
            r.settings.timeout = (ms > 0).then(|| std::time::Duration::from_millis(ms));
        }
    })
    .await;
    let report = Report::new(&collection, &unit, &iterations, planned, secret_vars);
    let mut entry = RunEntry::from_report(&report);
    entry.workspace = Some(space.id.clone());
    entry.target_key = format!("{}/{}", body.collection, report.key)
        .trim_end_matches('/')
        .to_owned();
    let id = state.store.add_run(owner, entry).await?;
    let mut summary = report.json(false);
    summary["id"] = json!(id);
    summary["collection"] = json!(body.collection);
    summary["workspace"] = json!(space.id);
    // A single-iteration flow run can be resumed at any step it reached.
    if let (snapreq_test::Unit::Flow(flow), [it]) = (&unit, iterations.as_slice())
        && let snapreq_test::UnitRun::Flow(run) = &it.result
    {
        let steps: Vec<usize> = run
            .states
            .iter()
            .map(|(n, _)| *n)
            .filter(|n| *n <= run.total)
            .collect();
        state
            .flow_states
            .insert(owner, id, &state_key, &flow.key, run.states.clone());
        summary["resumable_steps"] = json!(steps);
        summary["range"] = json!([run.range.0, run.range.1]);
    }
    Ok(summary)
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
struct LimitParams {
    limit: Option<usize>,
}

#[utoipa::path(get, path = "/runs", tag = "runs", params(LimitParams), responses((status = 200, description = "Run summaries, newest first", body = RunList)))]
async fn list_runs(
    State(state): Shared,
    identity: Identity,
    Query(p): Query<LimitParams>,
) -> ApiResult<Json<Value>> {
    let runs = state
        .store
        .runs(identity.owner(), p.limit.unwrap_or(50))
        .await?;
    Ok(Json(json!({ "runs": runs })))
}

#[utoipa::path(get, path = "/runs/{id}", tag = "runs", params(("id" = i64, Path, description = "Run id")), responses((status = 200, description = "The stored detailed report", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn get_run(
    State(state): Shared,
    identity: Identity,
    UrlPath(id): UrlPath<i64>,
) -> ApiResult<Json<Value>> {
    match state.store.run_report(identity.owner(), id).await? {
        Some(r) => Ok(Json(r)),
        None => Err(ApiError::not_found(format!("no run {id}"))),
    }
}

#[utoipa::path(get, path = "/runs/{id}/report/{format}", tag = "runs", params(("id" = i64, Path, description = "Run id"), ("format" = String, Path, description = "`json`, `junit` or `html`")), responses((status = 200, description = "The report as a file download (like `snapreq run --report`)"), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody)))]
/// `GET /api/v1/runs/{id}/report/{format}`: the report as a download.
async fn get_run_report(
    State(state): Shared,
    identity: Identity,
    UrlPath((id, format)): UrlPath<(i64, String)>,
) -> ApiResult<Response> {
    use crate::store::ReportFormat;
    let (fmt, mime, ext) = match format.as_str() {
        "json" => (ReportFormat::Json, "application/json", "json"),
        "junit" => (ReportFormat::Junit, "application/xml", "xml"),
        "html" => (ReportFormat::Html, "text/html; charset=utf-8", "html"),
        other => {
            return Err(ApiError::bad_request(format!(
                "unknown report format `{other}` (json, junit, html)"
            )));
        }
    };
    let text = state
        .store
        .run_rendering(identity.owner(), id, fmt)
        .await?
        .ok_or_else(|| ApiError::not_found(format!("no {format} report for run {id}")))?;
    let text = if fmt == ReportFormat::Json {
        serde_json::from_str::<Value>(&text)
            .and_then(|v| serde_json::to_string_pretty(&v))
            .unwrap_or(text)
    } else {
        text
    };
    // Always a download, never rendered in this origin (the HTML holds response bodies).
    Ok((
        [
            (axum::http::header::CONTENT_TYPE, mime.to_owned()),
            (
                axum::http::header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"snapreq-run-{id}.{ext}\""),
            ),
            (
                axum::http::header::CONTENT_SECURITY_POLICY,
                "sandbox".to_owned(),
            ),
        ],
        text,
    )
        .into_response())
}

// --- Collections -------------------------------------------------------------

#[utoipa::path(get, path = "/collections", tag = "collections", responses((status = 200, description = "Collections in the workspace", body = JsonDocument)))]
async fn list_collections(space: Space) -> ApiResult<Json<Value>> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&space.dir) {
        let mut names: Vec<String> = entries
            .filter_map(Result::ok)
            .filter(|e| e.path().join(COLLECTION_FILE).is_file())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| valid_segment(n))
            .collect();
        names.sort();
        for id in names {
            out.push(match Collection::load(space.dir.join(&id)) {
                Ok((c, _)) => json!({
                    "id": id,
                    "name": c.file.name,
                    "description": c.file.description,
                    "requests": c.requests().len(),
                    "flows": c.flows.len(),
                }),
                Err(e) => json!({ "id": id, "error": e.to_string() }),
            });
        }
    }
    Ok(Json(json!({ "collections": out })))
}

#[derive(Debug, Deserialize, utoipa::ToSchema)]
#[serde(deny_unknown_fields)]
struct NewCollection {
    id: String,
    name: Option<String>,
}

#[utoipa::path(post, path = "/collections", tag = "collections", request_body = NewCollection, responses((status = 201, description = "Created", body = JsonDocument), (status = 409, description = "Exists", body = ErrorBody), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn create_collection(
    space: EditSpace,
    Json(body): Json<NewCollection>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let dir = collection_dir(&space, &body.id)?;
    if dir.exists() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "conflict",
            format!("`{}` already exists", body.id),
        ));
    }
    let file = CollectionFile::new(body.name.unwrap_or_else(|| body.id.clone()));
    save_collection(&dir.join(COLLECTION_FILE), &file)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok((
        StatusCode::CREATED,
        Json(json!({ "id": body.id, "file": file })),
    ))
}

fn tree(items: &[Item], prefix: &str) -> Vec<Value> {
    items
        .iter()
        .map(|item| {
            let key = if prefix.is_empty() {
                item.key().to_owned()
            } else {
                format!("{prefix}/{}", item.key())
            };
            match item {
                Item::Request(r) => json!({
                    "type": "request",
                    "key": key,
                    "name": r.name(),
                    "method": r.file.method(),
                }),
                Item::Folder(f) => json!({
                    "type": "folder",
                    "key": key,
                    "name": f.file.name.as_deref().unwrap_or(&f.key),
                    "items": tree(&f.items, &key),
                }),
            }
        })
        .collect()
}

#[utoipa::path(get, path = "/collections/{id}", tag = "collections", params(("id" = String, Path, description = "Collection id")), responses((status = 200, description = "File, tree, flows and environments", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn get_collection(space: Space, UrlPath(id): UrlPath<String>) -> ApiResult<Json<Value>> {
    let c = load_collection(&space, &id)?;
    let environments = Environment::list(&c).map_err(|e| ApiError::invalid(e.to_string()))?;
    Ok(Json(json!({
        "id": id,
        "file": c.file,
        "items": tree(&c.items, ""),
        "flows": c.flows.iter().map(|f| json!({ "key": f.key, "name": f.name(), "steps": f.file.steps.len() })).collect::<Vec<_>>(),
        "environments": environments,
    })))
}

#[utoipa::path(put, path = "/collections/{id}", tag = "collections", params(("id" = String, Path, description = "Collection id")), request_body(content = Object, description = "`snapreq.toml` as JSON"), responses((status = 200, description = "Saved", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn put_collection(
    space: EditSpace,
    UrlPath(id): UrlPath<String>,
    Json(file): Json<CollectionFile>,
) -> ApiResult<Json<Value>> {
    let dir = collection_dir(&space, &id)?;
    if !dir.join(COLLECTION_FILE).is_file() {
        return Err(ApiError::not_found(format!("no collection `{id}`")));
    }
    if file.format > snapreq_collection::FORMAT_VERSION || file.format == 0 {
        return Err(ApiError::invalid(format!(
            "unsupported format {}",
            file.format
        )));
    }
    save_collection(&dir.join(COLLECTION_FILE), &file)
        .map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "id": id, "file": file })))
}

#[utoipa::path(delete, path = "/collections/{id}", tag = "collections", params(("id" = String, Path, description = "Collection id")), responses((status = 204, description = "Deleted"), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn delete_collection(
    space: EditSpace,
    UrlPath(id): UrlPath<String>,
) -> ApiResult<StatusCode> {
    let dir = collection_dir(&space, &id)?;
    if !dir.join(COLLECTION_FILE).is_file() {
        return Err(ApiError::not_found(format!("no collection `{id}`")));
    }
    std::fs::remove_dir_all(&dir).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

#[utoipa::path(get, path = "/collections/{id}/requests/{*key}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("key" = String, Path, description = "Request key, e.g. `users/create`")), responses((status = 200, description = "The request file as JSON", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn get_request(
    space: Space,
    UrlPath((id, key)): UrlPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = load_collection(&space, &id)?;
    let item = c
        .find(&key)
        .ok_or_else(|| ApiError::not_found(format!("no request `{key}`")))?;
    Ok(Json(
        json!({ "key": key.trim_matches('/'), "file": item.file }),
    ))
}

#[utoipa::path(put, path = "/collections/{id}/requests/{*key}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("key" = String, Path, description = "Request key")), request_body(content = Object, description = "A request file as JSON"), responses((status = 200, description = "Saved", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn put_request(
    space: EditSpace,
    UrlPath((id, key)): UrlPath<(String, String)>,
    Json(file): Json<RequestFile>,
) -> ApiResult<Json<Value>> {
    load_collection(&space, &id)?;
    let path = collection_dir(&space, &id)?.join(key_path(&key, ".toml")?);
    file.validate().map_err(ApiError::invalid)?;
    save_request(&path, &file).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "key": key.trim_matches('/'), "file": file })))
}

#[utoipa::path(delete, path = "/collections/{id}/requests/{*key}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("key" = String, Path, description = "Request key")), responses((status = 204, description = "Deleted"), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn delete_request(
    space: EditSpace,
    UrlPath((id, key)): UrlPath<(String, String)>,
) -> ApiResult<StatusCode> {
    load_collection(&space, &id)?;
    let root = collection_dir(&space, &id)?;
    let path = root.join(key_path(&key, ".toml")?);
    let status = remove_file(&path, &format!("request `{key}`"))?;
    let key = key.trim_matches('/');
    let (parent, name) = key.rsplit_once('/').unwrap_or(("", key));
    crate::tree::forget_in_order(&root, parent, name)?;
    Ok(status)
}

#[utoipa::path(get, path = "/collections/{id}/flows/{*key}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("key" = String, Path, description = "Flow key")), responses((status = 200, description = "The flow file as JSON", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn get_flow(
    space: Space,
    UrlPath((id, key)): UrlPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = load_collection(&space, &id)?;
    let flow = c
        .find_flow(&key)
        .ok_or_else(|| ApiError::not_found(format!("no flow `{key}`")))?;
    Ok(Json(json!({ "key": flow.key, "file": flow.file })))
}

#[utoipa::path(put, path = "/collections/{id}/flows/{*key}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("key" = String, Path, description = "Flow key")), request_body(content = Object, description = "A flow file as JSON"), responses((status = 200, description = "Saved", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn put_flow(
    space: EditSpace,
    UrlPath((id, key)): UrlPath<(String, String)>,
    Json(file): Json<FlowFile>,
) -> ApiResult<Json<Value>> {
    load_collection(&space, &id)?;
    let path = collection_dir(&space, &id)?.join(key_path(&key, snapreq_collection::FLOW_SUFFIX)?);
    file.validate().map_err(ApiError::invalid)?;
    save_flow(&path, &file).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "key": key.trim_matches('/'), "file": file })))
}

#[utoipa::path(delete, path = "/collections/{id}/flows/{*key}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("key" = String, Path, description = "Flow key")), responses((status = 204, description = "Deleted"), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn delete_flow(
    space: EditSpace,
    UrlPath((id, key)): UrlPath<(String, String)>,
) -> ApiResult<StatusCode> {
    load_collection(&space, &id)?;
    let path = collection_dir(&space, &id)?.join(key_path(&key, snapreq_collection::FLOW_SUFFIX)?);
    remove_file(&path, &format!("flow `{key}`"))
}

fn environment_path(space: &Space, id: &str, name: &str) -> ApiResult<PathBuf> {
    if !valid_segment(name) {
        return Err(ApiError::bad_request(format!(
            "invalid environment name `{name}`"
        )));
    }
    Ok(collection_dir(space, id)?
        .join(snapreq_collection::load::ENVIRONMENTS_DIR)
        .join(format!("{name}.toml")))
}

#[utoipa::path(get, path = "/collections/{id}/environments/{name}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("name" = String, Path, description = "Environment name")), responses((status = 200, description = "Variables (secret values never returned)", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn get_environment(
    space: Space,
    UrlPath((id, name)): UrlPath<(String, String)>,
) -> ApiResult<Json<Value>> {
    let c = load_collection(&space, &id)?;
    environment_path(&space, &id, &name)?;
    let env = Environment::load(&c, &name).map_err(|e| ApiError::not_found(e.to_string()))?;
    // Secret values are never serialized, only their markers.
    Ok(Json(json!({ "name": name, "vars": env.vars })))
}

#[utoipa::path(put, path = "/collections/{id}/environments/{name}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("name" = String, Path, description = "Environment name")), request_body(content = Object, description = "Variables: `{ \"name\": \"value\", \"token\": { \"secret\": true } }`"), responses((status = 200, description = "Saved", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn put_environment(
    space: EditSpace,
    UrlPath((id, name)): UrlPath<(String, String)>,
    Json(vars): Json<Variables>,
) -> ApiResult<Json<Value>> {
    load_collection(&space, &id)?;
    let path = environment_path(&space, &id, &name)?;
    if let Some(bad) = vars
        .0
        .iter()
        .find(|v| !snapreq_collection::vars::valid_name(&v.name))
    {
        return Err(ApiError::invalid(format!(
            "invalid variable name `{}`",
            bad.name
        )));
    }
    save_environment(&path, &vars).map_err(|e| ApiError::internal(e.to_string()))?;
    Ok(Json(json!({ "name": name, "vars": vars })))
}

#[utoipa::path(delete, path = "/collections/{id}/environments/{name}", tag = "collections", params(("id" = String, Path, description = "Collection id"), ("name" = String, Path, description = "Environment name")), responses((status = 204, description = "Deleted"), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
async fn delete_environment(
    space: EditSpace,
    UrlPath((id, name)): UrlPath<(String, String)>,
) -> ApiResult<StatusCode> {
    load_collection(&space, &id)?;
    let path = environment_path(&space, &id, &name)?;
    remove_file(&path, &format!("environment `{name}`"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_cannot_escape_or_collide() {
        assert_eq!(
            key_path("users/create-user", ".toml").unwrap(),
            Path::new("users").join("create-user.toml")
        );
        assert_eq!(
            key_path("/a/", ".flow.toml").unwrap(),
            PathBuf::from("a.flow.toml")
        );
        for bad in [
            "",
            "../x",
            "a/../b",
            "a//b",
            ".secrets/x",
            "environments/dev",
            "snapreq",
            "users/folder",
            "a/b.flow",
            "a b",
            "a\\b",
            "C:x",
        ] {
            assert!(key_path(bad, ".toml").is_err(), "{bad}");
        }
        assert!(valid_segment("my-api_v2.1"));
        assert!(!valid_segment(".."));
    }
}

#[derive(Debug, Deserialize, utoipa::IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in = Query)]
struct VariablesParams {
    /// Request (or flow) key whose scopes are listed; absent lists collection-level ones.
    key: Option<String>,
    /// Environment name.
    env: Option<String>,
}

#[utoipa::path(get, path = "/collections/{id}/variables", tag = "collections", params(("id" = String, Path, description = "Collection id"), VariablesParams), responses((status = 200, description = "`{ variables: [{ name, value, secret, has_value, scope }] }`; secret values are never returned", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Does not validate", body = ErrorBody)))]
/// The variables a request sees, with the scope each comes from (for highlighting
/// placeholders in the UI).
async fn list_variables(
    State(state): Shared,
    space: Space,
    UrlPath(id): UrlPath<String>,
    Query(p): Query<VariablesParams>,
) -> ApiResult<Json<Value>> {
    let c = load_collection(&space, &id)?;
    let environment = p
        .env
        .as_deref()
        .map(|e| Environment::load(&c, e))
        .transpose()
        .map_err(|e| ApiError::not_found(e.to_string()))?;
    let secrets =
        load_secrets(&state, &c, p.env.as_deref()).map_err(|e| ApiError::invalid(e.to_string()))?;
    let key = p.key.as_deref().unwrap_or("").trim_matches('/');
    let mut scopes = snapreq_collection::Scopes::for_request(
        &c,
        key,
        environment.as_ref(),
        &Variables::default(),
        &Variables::default(),
    );
    secrets.fill(&mut scopes);
    let variables: Vec<Value> = scopes
        .visible()
        .into_iter()
        .map(|(v, scope)| {
            json!({
                "name": v.name,
                "value": if v.secret { None } else { v.value.as_deref() },
                "secret": v.secret,
                "has_value": v.value.is_some(),
                "scope": scope.to_string(),
            })
        })
        .collect();
    Ok(Json(json!({ "variables": variables })))
}
