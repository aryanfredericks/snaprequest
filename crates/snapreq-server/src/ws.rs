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
//! The server answers with `trace` events (`{ "type": "trace", "id", "entry" }`, a redacted
//! trace entry), `progress` events of runs (`{ "type": "progress", "id", "event":
//! "request_started" | "request_done", ... }`) and finally `{ "type": "result", "id",
//! "result" }` (the same document as the REST endpoint) or `{ "type": "error", "id", "error" }`.
//! Jobs on one connection run one after another.

use std::sync::Arc;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
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

async fn serve(mut socket: WebSocket, state: Arc<AppState>, identity: Identity) {
    while let Some(Ok(msg)) = socket.recv().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            Message::Close(_) => break,
            // Pings are answered by axum; binary messages are not part of the protocol.
            _ => continue,
        };
        // A logout, password change or revoked token ends the connection.
        if !identity.still_valid(&state).await.unwrap_or(false) {
            let e = ApiError::new(
                axum::http::StatusCode::UNAUTHORIZED,
                "unauthorized",
                "the session has ended; log in again",
            );
            let _ = send(&mut socket, &error_message(&Value::Null, &e)).await;
            break;
        }
        if job(&mut socket, &state, &identity, &text).await.is_err() {
            break; // the client went away
        }
    }
}

fn error_message(id: &Value, e: &ApiError) -> Value {
    json!({ "type": "error", "id": id, "error": { "kind": e.kind, "message": e.message } })
}

async fn send(socket: &mut WebSocket, v: &Value) -> Result<(), axum::Error> {
    socket.send(Message::Text(v.to_string().into())).await
}

/// Runs one job, forwarding its live events while it runs.
async fn job(
    socket: &mut WebSocket,
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
    let kind = doc
        .as_object_mut()
        .and_then(|m| m.remove("type"))
        .and_then(|t| t.as_str().map(str::to_owned))
        .unwrap_or_default();

    let (tx, mut rx) = mpsc::unbounded_channel::<Value>();
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
                execute_body(state, owner, body, Some(trace)).await
            }
            "run" => {
                let body: RunBody = serde_json::from_value(doc).map_err(bad)?;
                let live = snapreq_test::Live {
                    trace: Some(trace),
                    progress: Some(progress),
                };
                run_body(state, owner, body, Some(live)).await
            }
            other => Err(ApiError::new(
                axum::http::StatusCode::BAD_REQUEST,
                "bad_request",
                format!("unknown message type `{other}` (expected `execute` or `run`)"),
            )),
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
