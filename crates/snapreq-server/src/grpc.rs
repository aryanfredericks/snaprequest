//! gRPC for the web UI (PLAN P10.4, ADR-036): `POST /api/v1/grpc/describe` lists a server's
//! services (reflection, or the request file's proto files), and `GET /api/v1/ws/grpc` runs a
//! call with live messages.
//!
//! The WebSocket's first message names the request (the fields of `POST /requests/execute`)
//! and the call:
//!
//! ```json
//! { "type": "start", "collection": "shop", "key": "orders/get", "env": "dev",
//!   "method": "shop.v1.Orders/Get", "messages": [{ "id": 7 }], "end": true }
//! ```
//!
//! `method` and `messages` default to the request file's `[grpc]` table. With `"end": false`
//! (client and bidirectional streaming) the client sends more with `{ "type": "send",
//! "message": {…} }` and ends with `{ "type": "end" }`; `{ "type": "cancel" }` stops the call.
//! The server sends `trace` events while connecting, then `{ "type": "headers", "metadata" }`,
//! `{ "type": "message", "at_ms", "message" }` for every response message, and finally
//! `{ "type": "status", "status": { "code", "name", "message" }, "trailers", "duration_ms" }` or
//! `{ "type": "error", "error" }`. Secret values are hidden in everything sent.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Json;
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use snapreq_collection::{Environment, GrpcCall, RequestFile};
use snapreq_core::Request;
use snapreq_grpc::{CallEvent, DescriptorPool};
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
    OpenApiRouter::new().routes(routes!(describe))
}

/// Which request (as for `/requests/execute`).
#[derive(Debug, Default, Deserialize, utoipa::ToSchema)]
struct Target {
    /// A complete request (the core JSON model, `url` like `grpc://host:port`). Services come
    /// from server reflection.
    #[schema(value_type = Option<Object>)]
    #[serde(default)]
    request: Option<Request>,
    /// Or a collection request: its `[grpc]` proto files are used, else reflection.
    #[serde(default)]
    collection: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[schema(value_type = Option<Object>)]
    #[serde(default)]
    file: Option<RequestFile>,
    #[serde(default)]
    env: Option<String>,
    #[serde(default)]
    vars: BTreeMap<String, String>,
}

/// A request ready to connect, with its resolved `[grpc]` table.
struct Prepared {
    request: Request,
    call: GrpcCall,
    hidden: Vec<String>,
    /// The workspace directory proto files must stay in.
    root: Option<PathBuf>,
}

async fn prepare(
    state: &AppState,
    identity: &Identity,
    ws: &str,
    t: Target,
) -> ApiResult<Prepared> {
    match (t.request, t.collection, t.key) {
        (Some(request), None, None) => {
            if t.env.is_some() || !t.vars.is_empty() || t.file.is_some() {
                return Err(ApiError::bad_request(
                    "`env`, `vars` and `file` only apply to collection requests",
                ));
            }
            let hidden = request.secrets();
            Ok(Prepared {
                request,
                call: GrpcCall {
                    method: None,
                    protos: Vec::new(),
                    import_paths: Vec::new(),
                    messages: Vec::new(),
                },
                hidden,
                root: None,
            })
        }
        (None, Some(cid), Some(key)) => {
            let space = Space::resolve(state, identity, ws).await?;
            if t.file.is_some() || !t.vars.is_empty() {
                space
                    .require(Role::Editor)
                    .map_err(|_| viewer_overrides(&space))?;
            }
            let mut collection = load_collection(&space, &cid)?;
            if let Some(file) = t.file {
                file.validate().map_err(ApiError::invalid)?;
                key_path(&key, ".toml")?;
                collection
                    .put_request(&key, file)
                    .map_err(ApiError::not_found)?;
            }
            let environment = t
                .env
                .as_deref()
                .map(|e| Environment::load(&collection, e))
                .transpose()
                .map_err(|e| ApiError::invalid(e.to_string()))?;
            let secrets = load_secrets(state, &collection, t.env.as_deref())
                .map_err(|e| ApiError::invalid(e.to_string()))?;
            let mut session = snapreq_test::Session::new(
                &collection,
                environment.as_ref(),
                &secrets,
                state.client.clone(),
            );
            for (k, v) in &t.vars {
                session.overrides.set(k, v);
            }
            let scopes = session.scopes(&key);
            for (k, v) in &t.vars {
                if scopes.is_declared_secret(k) {
                    session.hide(v.clone());
                }
            }
            let p = session.grpc(&key).await.map_err(ApiError::invalid)?;
            Ok(Prepared {
                request: p.request,
                call: p.call,
                hidden: p.hidden,
                root: Some(space.dir.clone()),
            })
        }
        _ => Err(ApiError::bad_request(
            "give either `request`, or `collection` and `key`",
        )),
    }
}

/// Proto files and import paths must be inside the workspace.
fn inside(root: &Path, paths: &[PathBuf]) -> ApiResult<()> {
    let root = root
        .canonicalize()
        .map_err(|e| ApiError::internal(e.to_string()))?;
    for p in paths {
        let real = p
            .canonicalize()
            .map_err(|e| ApiError::invalid(format!("`{}`: {e}", p.display())))?;
        if !real.starts_with(&root) {
            return Err(ApiError::invalid(format!(
                "`{}` is outside the workspace",
                p.display()
            )));
        }
    }
    Ok(())
}

async fn connect(
    state: &AppState,
    p: &Prepared,
    sink: Option<snapreq_core::TraceSink>,
) -> ApiResult<(snapreq_grpc::Channel, DescriptorPool)> {
    let hide = |e: &dyn std::fmt::Display| {
        ApiError::invalid(snapreq_test::report::hide(&e.to_string(), &p.hidden))
    };
    let channel = snapreq_grpc::connect(&state.client, &p.request, sink)
        .await
        .map_err(|e| hide(&e))?;
    let pool = if p.call.protos.is_empty() {
        snapreq_grpc::reflect(&channel)
            .await
            .map_err(|e| hide(&e))?
    } else {
        if let Some(root) = &p.root {
            inside(root, &p.call.protos)?;
            inside(root, &p.call.import_paths)?;
        }
        snapreq_grpc::compile_protos(&p.call.protos, &p.call.import_paths).map_err(|e| hide(&e))?
    };
    Ok((channel, pool))
}

#[utoipa::path(post, path = "/grpc/describe", tag = "requests", request_body = Target, responses((status = 200, description = "`{ services: [{ name, methods: [{ name, path, input, output, client_streaming, server_streaming, template }] }] }`", body = JsonDocument), (status = 400, description = "Bad request", body = ErrorBody), (status = 404, description = "Not found", body = ErrorBody), (status = 422, description = "Cannot connect, or no descriptions", body = ErrorBody)))]
/// `POST /api/v1/grpc/describe`
async fn describe(
    State(state): State<Arc<AppState>>,
    identity: Identity,
    headers: axum::http::HeaderMap,
    Json(target): Json<Target>,
) -> ApiResult<Json<Value>> {
    let ws = crate::workspaces::requested(&headers);
    let p = prepare(&state, &identity, &ws, target).await?;
    let (_, pool) = connect(&state, &p, None).await?;
    Ok(Json(json!({ "services": snapreq_grpc::describe(&pool) })))
}

#[derive(Debug, Deserialize)]
struct Start {
    #[allow(dead_code)]
    r#type: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(flatten)]
    target: Target,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    messages: Option<Vec<Value>>,
    #[serde(default = "yes")]
    end: bool,
}

fn yes() -> bool {
    true
}

pub async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    identity: Identity,
) -> Response {
    ws.on_upgrade(move |socket| serve(socket, state, identity))
}

async fn send(socket: &mut WebSocket, v: &Value) -> bool {
    socket
        .send(Message::Text(v.to_string().into()))
        .await
        .is_ok()
}

fn error(e: &ApiError) -> Value {
    json!({ "type": "error", "error": { "kind": e.kind, "message": e.message } })
}

fn text_of(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

async fn serve(mut socket: WebSocket, state: Arc<AppState>, identity: Identity) {
    let first = loop {
        match socket.recv().await {
            Some(Ok(Message::Text(t))) => break t.to_string(),
            Some(Ok(Message::Close(_)) | Err(_)) | None => return,
            Some(Ok(_)) => {}
        }
    };
    let start: Start = match serde_json::from_str(&first) {
        Ok(s) => s,
        Err(e) => {
            send(&mut socket, &error(&ApiError::bad_request(e.to_string()))).await;
            return;
        }
    };
    let ws = start
        .workspace
        .clone()
        .unwrap_or_else(|| crate::workspaces::DEFAULT.to_owned());
    if let Err(e) = run(&mut socket, &state, &identity, &ws, start).await {
        send(&mut socket, &error(&e)).await;
    }
}

async fn run(
    socket: &mut WebSocket,
    state: &AppState,
    identity: &Identity,
    ws: &str,
    start: Start,
) -> ApiResult<()> {
    let p = prepare(state, identity, ws, start.target).await?;
    let hidden = p.hidden.clone();
    let shown = |v: Value| snapreq_test::report::hide_json(v, &hidden);
    // Trace events while connecting.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let trace_tx = tx.clone();
    let sink: snapreq_core::TraceSink = Arc::new(move |entry| {
        let _ = trace_tx.send(json!({ "type": "trace", "entry": entry }));
    });
    let connecting = connect(state, &p, Some(sink));
    tokio::pin!(connecting);
    let (channel, pool) = loop {
        tokio::select! {
            biased;
            Some(v) = rx.recv() => { send(socket, &v).await; }
            r = &mut connecting => break r?,
        }
    };
    while let Ok(v) = rx.try_recv() {
        send(socket, &v).await;
    }
    let name = start
        .method
        .or(p.call.method.clone())
        .ok_or_else(|| ApiError::bad_request("name the method to call (package.Service/Method)"))?;
    let method =
        snapreq_grpc::find_method(&pool, &name).map_err(|e| ApiError::invalid(e.to_string()))?;
    let mut first: Vec<String> = match start.messages {
        Some(list) => list.iter().map(text_of).collect(),
        None => p.call.messages.clone(),
    };
    if first.is_empty() && !method.is_client_streaming() {
        first.push("{}".into());
    }
    let (in_tx, in_rx) = tokio::sync::mpsc::unbounded_channel::<snapreq_grpc::DynamicMessage>();
    for t in &first {
        let m = snapreq_grpc::message_from_json(&method.input(), t)
            .map_err(|e| ApiError::invalid(e.to_string()))?;
        let _ = in_tx.send(m);
    }
    let mut in_tx = (!start.end && method.is_client_streaming()).then_some(in_tx);
    let inputs =
        futures_util::stream::unfold(
            in_rx,
            |mut rx| async move { rx.recv().await.map(|m| (m, rx)) },
        );
    let event_tx = tx.clone();
    drop(tx);
    let calling = snapreq_grpc::call(&channel, &method, inputs, move |e| {
        let v = match e {
            CallEvent::Headers(h) => json!({ "type": "headers", "metadata": h }),
            CallEvent::Message { at, message } => {
                json!({ "type": "message", "at_ms": at.as_secs_f64() * 1000.0, "message": message })
            }
        };
        let _ = event_tx.send(v);
    });
    tokio::pin!(calling);
    let outcome = loop {
        tokio::select! {
            biased;
            Some(v) = rx.recv() => {
                if !send(socket, &shown(v)).await {
                    return Ok(());
                }
            }
            r = &mut calling => break r,
            m = socket.recv() => {
                let text = match m {
                    Some(Ok(Message::Text(t))) => t.to_string(),
                    Some(Ok(Message::Close(_)) | Err(_)) | None => return Ok(()),
                    Some(Ok(_)) => continue,
                };
                let cmd: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
                match cmd["type"].as_str() {
                    Some("send") => match (&in_tx, snapreq_grpc::message_from_json(&method.input(), &text_of(&cmd["message"]))) {
                        (Some(tx), Ok(m)) => { let _ = tx.send(m); }
                        (None, _) => { send(socket, &error(&ApiError::bad_request("the request stream has ended"))).await; }
                        (_, Err(e)) => { send(socket, &error(&ApiError::invalid(e.to_string()))).await; }
                    },
                    Some("end") => in_tx = None,
                    Some("cancel") => {
                        let status = json!({ "type": "status", "status": { "code": 1, "name": "CANCELLED", "message": "cancelled by the client" }, "trailers": {}, "duration_ms": null });
                        send(socket, &status).await;
                        return Ok(());
                    }
                    _ => { send(socket, &error(&ApiError::bad_request("expected `send`, `end` or `cancel`"))).await; }
                }
            }
        }
    };
    while let Ok(v) = rx.try_recv() {
        send(socket, &shown(v)).await;
    }
    let outcome = outcome
        .map_err(|e| ApiError::invalid(snapreq_test::report::hide(&e.to_string(), &hidden)))?;
    let doc = json!({
        "type": "status",
        "status": outcome.status,
        "trailers": outcome.trailers,
        "duration_ms": outcome.duration.as_secs_f64() * 1000.0,
    });
    send(socket, &shown(doc)).await;
    Ok(())
}
