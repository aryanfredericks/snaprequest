//! `GET /api/v1/ws/websocket`: a WebSocket connection to a target, relayed for the web UI
//! (PLAN P10.1, ADR-033). The server opens the connection, so the outbound policy, secrets
//! and auth apply as for any request.
//!
//! The client's first message says what to open:
//!
//! ```json
//! { "type": "connect", "workspace": "default", "collection": "shop", "key": "chat", "env": "dev" }
//! { "type": "connect", "request": { "method": "GET", "url": "wss://echo.example/ws" }, "subprotocols": ["chat"] }
//! ```
//!
//! (`file` and `vars` as in `POST /requests/execute`, for editors.) Then:
//! `{ "type": "send", "message": { "kind": "text", "data": "hi" } }`, `{ "type": "send", "name":
//! "join" }` (a saved message of the request file, resolved by the server) and `{ "type": "close" }`.
//!
//! The server answers with `trace` events of the handshake, `{ "type": "open", "handshake",
//! "protocol", "messages": [names] }`, `{ "type": "message", "direction": "sent" | "received",
//! "at_ms", "message" }` for every message, and finally `{ "type": "closed", "by", "code",
//! "reason" }` or `{ "type": "error", "error" }`. Secret values are hidden in everything sent
//! to the client.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::Response;
use serde::Deserialize;
use serde_json::{Value, json};
use snapreq_collection::{Environment, RequestFile};
use snapreq_core::Request;
use snapreq_core::client::ws::{WsMessage, WsOptions};

use crate::AppState;
use crate::api::{ApiError, load_collection, load_secrets, viewer_overrides};
use crate::auth::Identity;
use crate::workspaces::{Role, Space};

/// The first message of a session.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Connect {
    #[allow(dead_code)]
    r#type: String,
    #[serde(default)]
    workspace: Option<String>,
    #[serde(default)]
    request: Option<Request>,
    #[serde(default)]
    subprotocols: Vec<String>,
    #[serde(default)]
    collection: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    file: Option<RequestFile>,
    #[serde(default)]
    env: Option<String>,
    #[serde(default)]
    vars: BTreeMap<String, String>,
}

/// Later messages.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Command {
    Send {
        #[serde(default)]
        message: Option<WsMessage>,
        #[serde(default)]
        name: Option<String>,
    },
    Close {
        #[serde(default)]
        code: Option<u16>,
        #[serde(default)]
        reason: String,
    },
}

struct Opened {
    request: Request,
    options: WsOptions,
    saved: Vec<(String, WsMessage)>,
    hidden: Vec<String>,
}

pub async fn upgrade(
    ws: WebSocketUpgrade,
    State(state): State<Arc<AppState>>,
    identity: Identity,
) -> Response {
    ws.on_upgrade(move |socket| serve(socket, state, identity))
}

async fn send(socket: &mut WebSocket, v: &Value) -> Result<(), axum::Error> {
    socket.send(Message::Text(v.to_string().into())).await
}

fn error(e: &ApiError) -> Value {
    json!({ "type": "error", "error": { "kind": e.kind, "message": e.message } })
}

async fn serve(mut socket: WebSocket, state: Arc<AppState>, identity: Identity) {
    let first = loop {
        match socket.recv().await {
            Some(Ok(Message::Text(t))) => break t.to_string(),
            Some(Ok(Message::Close(_)) | Err(_)) | None => return,
            Some(Ok(_)) => {}
        }
    };
    let connect: Connect = match serde_json::from_str(&first) {
        Ok(c) => c,
        Err(e) => {
            let _ = send(&mut socket, &error(&ApiError::bad_request(e.to_string()))).await;
            return;
        }
    };
    let opened = match prepare(&state, &identity, connect).await {
        Ok(o) => o,
        Err(e) => {
            let _ = send(&mut socket, &error(&e)).await;
            return;
        }
    };
    relay(socket, &state, opened).await;
}

/// Builds the request to open (collection requests with scripts and variables).
async fn prepare(state: &AppState, identity: &Identity, c: Connect) -> Result<Opened, ApiError> {
    match (c.request, c.collection, c.key) {
        (Some(request), None, None) => {
            if c.env.is_some() || !c.vars.is_empty() || c.file.is_some() {
                return Err(ApiError::bad_request(
                    "`env`, `vars` and `file` only apply to collection requests",
                ));
            }
            request
                .effective_ws_url()
                .map_err(|e| ApiError::invalid(e.to_string()))?;
            let hidden = request.secrets();
            Ok(Opened {
                request,
                options: WsOptions {
                    subprotocols: c.subprotocols,
                },
                saved: Vec::new(),
                hidden,
            })
        }
        (None, Some(cid), Some(key)) => {
            let ws = c
                .workspace
                .unwrap_or_else(|| crate::workspaces::DEFAULT.to_owned());
            let space = Space::resolve(state, identity, &ws).await?;
            if c.file.is_some() || !c.vars.is_empty() {
                space
                    .require(Role::Editor)
                    .map_err(|_| viewer_overrides(&space))?;
            }
            let mut collection = load_collection(&space, &cid)?;
            if let Some(file) = c.file {
                file.validate().map_err(ApiError::invalid)?;
                crate::api::key_path(&key, ".toml")?;
                collection
                    .put_request(&key, file)
                    .map_err(ApiError::not_found)?;
            }
            let environment = c
                .env
                .as_deref()
                .map(|e| Environment::load(&collection, e))
                .transpose()
                .map_err(|e| ApiError::invalid(e.to_string()))?;
            let secrets = load_secrets(state, &collection, c.env.as_deref())
                .map_err(|e| ApiError::invalid(e.to_string()))?;
            let mut session = snapreq_test::Session::new(
                &collection,
                environment.as_ref(),
                &secrets,
                state.client.clone(),
            );
            for (k, v) in &c.vars {
                session.overrides.set(k, v);
            }
            let scopes = session.scopes(&key);
            for (k, v) in &c.vars {
                if scopes.is_declared_secret(k) {
                    session.hide(v.clone());
                }
            }
            let mut p = session.websocket(&key).await.map_err(ApiError::invalid)?;
            if !c.subprotocols.is_empty() {
                p.options.subprotocols = c.subprotocols;
            }
            Ok(Opened {
                request: p.request,
                options: p.options,
                saved: p.messages,
                hidden: p.hidden,
            })
        }
        _ => Err(ApiError::bad_request(
            "give either `request`, or `collection` and `key`",
        )),
    }
}

/// A message for the client, secret values hidden.
fn shown(m: &WsMessage, hidden: &[String]) -> Value {
    let mut v = json!(m);
    for field in ["data", "reason"] {
        if let Some(text) = v.get(field).and_then(Value::as_str)
            && matches!(m, WsMessage::Text { .. } | WsMessage::Close { .. })
        {
            v[field] = json!(snapreq_test::report::hide(text, hidden));
        }
    }
    v
}

async fn relay(mut socket: WebSocket, state: &AppState, o: Opened) {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let trace: snapreq_core::TraceSink = Arc::new(move |entry| {
        let _ = tx.send(json!({ "type": "trace", "entry": entry }));
    });
    let opening = state.client.websocket(&o.request, &o.options, Some(trace));
    tokio::pin!(opening);
    let opened = loop {
        tokio::select! {
            biased;
            Some(event) = rx.recv() => {
                if send(&mut socket, &event).await.is_err() {
                    return;
                }
            }
            r = &mut opening => break r,
        }
    };
    while let Ok(event) = rx.try_recv() {
        let _ = send(&mut socket, &event).await;
    }
    let target = match opened {
        Ok(t) => t,
        Err(e) => {
            let mut doc = json!({ "type": "error", "error": { "kind": "execution", "message": e.to_string() } });
            if let Some(trace) = e.trace() {
                doc["error"]["trace"] = json!(trace);
            }
            let _ = send(&mut socket, &doc).await;
            return;
        }
    };
    let start = Instant::now();
    let mut handshake = json!(target.handshake);
    handshake["url"] = json!(snapreq_test::report::hide(&target.handshake.url, &o.hidden));
    let names: Vec<&str> = o.saved.iter().map(|(n, _)| n.as_str()).collect();
    let open = json!({ "type": "open", "handshake": handshake, "protocol": target.protocol, "messages": names });
    if send(&mut socket, &open).await.is_err() {
        return;
    }
    let (mut out, mut incoming) = target.split();
    let message = |direction: &str, m: &WsMessage| {
        json!({
            "type": "message",
            "direction": direction,
            "at_ms": start.elapsed().as_secs_f64() * 1000.0,
            "message": shown(m, &o.hidden),
        })
    };
    loop {
        tokio::select! {
            m = incoming.recv() => match m {
                Some(Ok(m)) => {
                    if send(&mut socket, &message("received", &m)).await.is_err() {
                        let _ = out.close(Some(1001), "").await;
                        return;
                    }
                    if let WsMessage::Close { code, reason } = &m {
                        let _ = out.close(*code, "").await;
                        let reason = snapreq_test::report::hide(reason, &o.hidden);
                        let _ = send(&mut socket, &json!({ "type": "closed", "by": "server", "code": code, "reason": reason })).await;
                        return;
                    }
                }
                Some(Err(e)) => {
                    let msg = snapreq_test::report::hide(&e.to_string(), &o.hidden);
                    let _ = send(&mut socket, &json!({ "type": "error", "error": { "kind": "websocket", "message": msg } })).await;
                    return;
                }
                None => {
                    let _ = send(&mut socket, &json!({ "type": "closed", "by": "server", "code": null, "reason": "" })).await;
                    return;
                }
            },
            c = socket.recv() => {
                let text = match c {
                    Some(Ok(Message::Text(t))) => t.to_string(),
                    Some(Ok(Message::Close(_)) | Err(_)) | None => {
                        let _ = out.close(Some(1001), "").await;
                        return;
                    }
                    Some(Ok(_)) => continue,
                };
                let command: Command = match serde_json::from_str(&text) {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = send(&mut socket, &error(&ApiError::bad_request(e.to_string()))).await;
                        continue;
                    }
                };
                match command {
                    Command::Send { message: m, name } => {
                        let m = match (m, name) {
                            (Some(m), None) => m,
                            (None, Some(name)) => match o.saved.iter().find(|(n, _)| *n == name) {
                                Some((_, m)) => m.clone(),
                                None => {
                                    let e = ApiError::bad_request(format!("no saved message `{name}`"));
                                    let _ = send(&mut socket, &error(&e)).await;
                                    continue;
                                }
                            },
                            _ => {
                                let e = ApiError::bad_request("give either `message` or `name`");
                                let _ = send(&mut socket, &error(&e)).await;
                                continue;
                            }
                        };
                        if let Err(e) = out.send(m.clone()).await {
                            let msg = snapreq_test::report::hide(&e.to_string(), &o.hidden);
                            let _ = send(&mut socket, &json!({ "type": "error", "error": { "kind": "websocket", "message": msg } })).await;
                            return;
                        }
                        if send(&mut socket, &message("sent", &m)).await.is_err() {
                            let _ = out.close(Some(1001), "").await;
                            return;
                        }
                    }
                    Command::Close { code, reason } => {
                        let _ = out.close(code, &reason).await;
                        // Wait briefly for the server's answer, then report.
                        let wait = tokio::time::timeout(std::time::Duration::from_secs(2), async {
                            while let Some(Ok(m)) = incoming.recv().await {
                                if let WsMessage::Close { code, .. } = m {
                                    return code;
                                }
                            }
                            None
                        })
                        .await
                        .ok()
                        .flatten();
                        let _ = send(&mut socket, &json!({ "type": "closed", "by": "client", "code": code.or(wait).or(Some(1000)), "reason": reason })).await;
                        return;
                    }
                }
            }
        }
    }
}
