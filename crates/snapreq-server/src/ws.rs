//! `GET /api/v1/ws`: executes requests and runs with live trace and progress events.
//!
//! The client sends one JSON message per job; `id` is optional and echoed on every message
//! about that job:
//!
//! ```json
//! { "type": "execute", "id": 1, "request": { "method": "GET", "url": "https://example.com" } }
//! { "type": "run", "id": 2, "collection": "shop", "target": "orders/checkout" }
//! ```
//!
//! Collections come from the workspace named by an optional `"workspace"` field (default
//! `default`), which the caller must belong to.
//!
//! The server answers with `trace` events (`{ "type": "trace", "id", "entry" }`, a redacted
//! trace entry), `progress` events of runs (`{ "type": "progress", "id", "event":
//! "request_started" | "request_done", ... }`) and finally `{ "type": "result", "id",
//! "result" }` (the same document as the REST endpoint) or `{ "type": "error", "id", "error" }`.
//! Executions whose response is a `text/event-stream` also send `{ "type": "event", "id",
//! "event": { "event", "data", "id", "retry", "at_ms" } }` for every event as it arrives.
//! `{ "type": "cancel", "id" }` stops a running execution (it ends with what it read so far).
//! Jobs on one connection run one after another; messages sent meanwhile wait their turn.

use std::collections::VecDeque;
use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use crate::AppState;
use crate::api::{ApiError, ExecuteBody, RunBody, execute_body, run_body};
use crate::auth::Identity;

pub async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    identity: Identity,
) -> Response {
    ws.on_upgrade(move |socket| serve(socket, state, identity))
}

type Tx = SplitSink<WebSocket, Message>;
type Rx = SplitStream<WebSocket>;

async fn serve(socket: WebSocket, state: Arc<AppState>, identity: Identity) {
    let (mut tx, mut rx) = socket.split();
    let mut queue: VecDeque<String> = VecDeque::new();
    loop {
        let text = match queue.pop_front() {
            Some(t) => t,
            None => match rx.next().await {
                Some(Ok(Message::Text(t))) => t.to_string(),
                Some(Ok(Message::Close(_)) | Err(_)) | None => break,
                // Pings are answered by axum; binary messages are not part of the protocol.
                Some(Ok(_)) => continue,
            },
        };
        // A logout, password change or revoked token ends the connection.
        if !identity.still_valid(&state).await.unwrap_or(false) {
            let e = ApiError::new(
                axum::http::StatusCode::UNAUTHORIZED,
                "unauthorized",
                "the session has ended; log in again",
            );
            let _ = send(&mut tx, &error_message(&Value::Null, &e)).await;
            break;
        }
        if job(&mut tx, &mut rx, &mut queue, &state, &identity, &text)
            .await
            .is_err()
        {
            break; // the client went away
        }
    }
}

fn error_message(id: &Value, e: &ApiError) -> Value {
    json!({ "type": "error", "id": id, "error": { "kind": e.kind, "message": e.message } })
}

async fn send(socket: &mut Tx, v: &Value) -> Result<(), axum::Error> {
    socket.send(Message::Text(v.to_string().into())).await
}

/// A `{ "type": "cancel", "id" }` message: the id it cancels.
fn cancel_of(text: &str) -> Option<Value> {
    let v: Value = serde_json::from_str(text).ok()?;
    (v["type"] == "cancel").then(|| v["id"].clone())
}

/// Runs one job, forwarding its live events while it runs.
async fn job(
    socket: &mut Tx,
    incoming: &mut Rx,
    queue: &mut VecDeque<String>,
    state: &AppState,
    identity: &Identity,
    text: &str,
) -> Result<(), axum::Error> {
    let owner = identity.owner();
    let mut doc: Value = match serde_json::from_str(text) {
        Ok(Value::Object(m)) => Value::Object(m),
        _ => {
            let e = ApiError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "bad_request",
                "expected a JSON object",
            );
            return send(socket, &error_message(&Value::Null, &e)).await;
        }
    };
    let id = doc
        .as_object_mut()
        .and_then(|m| m.remove("id"))
        .unwrap_or(Value::Null);
    let ws = doc
        .as_object_mut()
        .and_then(|m| m.remove("workspace"))
        .and_then(|w| w.as_str().map(str::to_owned))
        .unwrap_or_else(|| crate::workspaces::DEFAULT.to_owned());
    let kind = doc
        .as_object_mut()
        .and_then(|m| m.remove("type"))
        .and_then(|t| t.as_str().map(str::to_owned))
        .unwrap_or_default();

    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
    let event_tx = tx.clone();
    let event_id = id.clone();
    let streaming = snapreq_core::Streaming::new(snapreq_core::sse::observer(move |e| {
        let _ = event_tx.send(json!({ "type": "event", "id": event_id, "event": e }));
    }));
    let stop = streaming.stop.clone();
    let trace_tx = tx.clone();
    let trace_id = id.clone();
    let trace: snapreq_core::TraceSink = Arc::new(move |entry| {
        let _ = trace_tx.send(json!({ "type": "trace", "id": trace_id, "entry": entry }));
    });
    let progress_id = id.clone();
    let progress: snapreq_test::session::ProgressSink = Arc::new(move |event| {
        let mut v = json!(event);
        v["type"] = json!("progress");
        v["id"] = progress_id.clone();
        let _ = tx.send(v);
    });

    let bad = |e: serde_json::Error| {
        ApiError::new(
            axum::http::StatusCode::BAD_REQUEST,
            "bad_request",
            e.to_string(),
        )
    };
    let work = async move {
        match kind.as_str() {
            "execute" => {
                drop(progress);
                let body: ExecuteBody = serde_json::from_value(doc).map_err(bad)?;
                execute_body(state, identity, &ws, body, Some(trace), Some(streaming)).await
            }
            "run" => {
                drop(streaming);
                let body: RunBody = serde_json::from_value(doc).map_err(bad)?;
                let live = snapreq_test::Live {
                    trace: Some(trace),
                    progress: Some(progress),
                };
                let space = crate::workspaces::Space::resolve(state, identity, &ws).await?;
                run_body(state, &space, owner, body, Some(live)).await
            }
            other => {
                drop((streaming, progress, trace));
                Err(ApiError::new(
                    axum::http::StatusCode::BAD_REQUEST,
                    "bad_request",
                    format!("unknown message type `{other}` (expected `execute` or `run`)"),
                ))
            }
        }
        // Every sender is dropped here, which ends the forwarding loop below.
    };

    let mut work = std::pin::pin!(work);
    let mut result = None;
    let mut closed = false;
    // Until the job is done and every event is forwarded. The senders can all be gone
    // before the job finishes (it still stores history), so a closed channel must stop
    // being polled, or it would win every round and starve the job.
    while result.is_none() || !closed {
        tokio::select! {
            // Live events first, so the result never overtakes them.
            biased;
            event = rx.recv(), if !closed => match event {
                Some(v) => send(socket, &v).await?,
                None => closed = true,
            },
            r = &mut work, if result.is_none() => result = Some(r),
            m = incoming.next(), if result.is_none() => match m {
                Some(Ok(Message::Text(t))) => {
                    let t = t.to_string();
                    match cancel_of(&t) {
                        Some(cancelled) if cancelled == id => stop.cancel(),
                        // Cancelling a job that is not running does nothing.
                        Some(_) => {}
                        None => queue.push_back(t),
                    }
                }
                Some(Ok(Message::Close(_)) | Err(_)) | None => {
                    stop.cancel();
                    return Err(axum::Error::new("the client went away"));
                }
                Some(Ok(_)) => {}
            },
        }
    }
    let message = match result.unwrap_or_else(|| {
        Err(ApiError::new(
            axum::http::StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "no result",
        ))
    }) {
        Ok(v) => json!({ "type": "result", "id": id, "result": v }),
        Err(e) => error_message(&id, &e),
    };
    send(socket, &message).await
}
