//! Calls: messages in, messages out, then a status (any kind of method, one code path).

use std::time::{Duration, Instant};

use futures_util::Stream;
use prost_reflect::{DynamicMessage, MethodDescriptor};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tonic::metadata::{KeyAndValueRef, MetadataKey, MetadataMap, MetadataValue};

use crate::channel::Channel;
use crate::codec::DynamicCodec;
use crate::describe::message_to_json;
use crate::{Error, Result};

/// The name of a gRPC status code (`NOT_FOUND`).
pub fn status_name(code: i32) -> &'static str {
    match code {
        0 => "OK",
        1 => "CANCELLED",
        2 => "UNKNOWN",
        3 => "INVALID_ARGUMENT",
        4 => "DEADLINE_EXCEEDED",
        5 => "NOT_FOUND",
        6 => "ALREADY_EXISTS",
        7 => "PERMISSION_DENIED",
        8 => "RESOURCE_EXHAUSTED",
        9 => "FAILED_PRECONDITION",
        10 => "ABORTED",
        11 => "OUT_OF_RANGE",
        12 => "UNIMPLEMENTED",
        13 => "INTERNAL",
        14 => "UNAVAILABLE",
        15 => "DATA_LOSS",
        16 => "UNAUTHENTICATED",
        _ => "UNKNOWN_CODE",
    }
}

/// The status a call ended with.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GrpcStatus {
    pub code: i32,
    pub name: String,
    pub message: String,
}

impl GrpcStatus {
    fn of(s: &tonic::Status) -> Self {
        let code = s.code() as i32;
        Self {
            code,
            name: status_name(code).into(),
            message: s.message().to_owned(),
        }
    }

    pub fn ok(&self) -> bool {
        self.code == 0
    }
}

/// Something that happened during a call, as it happened.
#[derive(Debug, Clone)]
pub enum CallEvent {
    /// The response headers (initial metadata).
    Headers(Value),
    /// A response message, with the time since the call started.
    Message { at: Duration, message: Value },
}

/// A finished call.
#[derive(Debug, Clone, Serialize)]
pub struct CallOutcome {
    pub headers: Value,
    pub messages: Vec<Value>,
    pub status: GrpcStatus,
    pub trailers: Value,
    #[serde(rename = "duration_ms", serialize_with = "ms")]
    pub duration: Duration,
}

fn ms<S: serde::Serializer>(d: &Duration, s: S) -> std::result::Result<S::Ok, S::Error> {
    s.serialize_f64(d.as_secs_f64() * 1000.0)
}

/// Metadata as a JSON object (`name: value`, repeated names as arrays; binary values by size).
pub fn metadata_json(map: &MetadataMap) -> Value {
    let mut out = Map::new();
    for kv in map.iter() {
        let (name, value) = match kv {
            KeyAndValueRef::Ascii(k, v) => (
                k.as_str().to_owned(),
                json!(v.to_str().unwrap_or("<not text>")),
            ),
            KeyAndValueRef::Binary(k, v) => (
                k.as_str().to_owned(),
                json!(format!(
                    "<{} bytes>",
                    v.to_bytes().map(|b| b.len()).unwrap_or(0)
                )),
            ),
        };
        match out.get_mut(&name) {
            Some(Value::Array(a)) => a.push(value),
            Some(existing) => *existing = json!([existing.clone(), value]),
            None => {
                out.insert(name, value);
            }
        }
    }
    Value::Object(out)
}

/// Adds request metadata (`-bin` names are not supported).
pub(crate) fn add_metadata(map: &mut MetadataMap, metadata: &[(String, String)]) -> Result<()> {
    for (name, value) in metadata {
        if name.ends_with("-bin") {
            return Err(Error::Invalid(format!(
                "binary metadata `{name}` is not supported"
            )));
        }
        let key = MetadataKey::from_bytes(name.as_bytes())
            .map_err(|_| Error::Invalid(format!("invalid metadata name `{name}`")))?;
        let value: MetadataValue<_> = value
            .parse()
            .map_err(|_| Error::Invalid(format!("invalid value for metadata `{name}`")))?;
        map.append(key, value);
    }
    Ok(())
}

/// Calls `method` with the messages of `inputs` (the stream ends the request side when it ends),
/// reporting headers and messages to `on_event` as they arrive. A non-OK status is a normal
/// outcome; `Err` is for calls that could not be made.
pub async fn call<S>(
    channel: &Channel,
    method: &MethodDescriptor,
    inputs: S,
    mut on_event: impl FnMut(CallEvent) + Send,
) -> Result<CallOutcome>
where
    S: Stream<Item = DynamicMessage> + Send + 'static,
{
    let mut grpc = tonic::client::Grpc::with_origin(channel.send.clone(), channel.origin.clone());
    grpc.ready()
        .await
        .map_err(|e| Error::Invalid(format!("the connection is not usable: {e}")))?;
    let path: http::uri::PathAndQuery =
        format!("/{}/{}", method.parent_service().full_name(), method.name())
            .parse()
            .map_err(|e| Error::Invalid(format!("bad method path: {e}")))?;
    let mut request = tonic::Request::new(inputs);
    add_metadata(request.metadata_mut(), &channel.metadata)?;
    if let Some(t) = channel.timeout {
        request.set_timeout(t);
    }
    let started = Instant::now();
    let codec = DynamicCodec::new(method.output());
    let mut messages = Vec::new();
    let mut headers = Value::Object(Map::new());
    let work = async {
        let response = match grpc.streaming(request, path, codec).await {
            Ok(r) => r,
            Err(status) => {
                return (
                    metadata_json(status.metadata()),
                    GrpcStatus::of(&status),
                    Value::Object(Map::new()),
                );
            }
        };
        let head = metadata_json(response.metadata());
        on_event(CallEvent::Headers(head.clone()));
        let mut stream = response.into_inner();
        loop {
            match stream.message().await {
                Ok(Some(m)) => {
                    let json = message_to_json(&m);
                    on_event(CallEvent::Message {
                        at: started.elapsed(),
                        message: json.clone(),
                    });
                    messages.push(json);
                }
                Ok(None) => break,
                Err(status) => {
                    return (
                        head,
                        GrpcStatus::of(&status),
                        metadata_json(status.metadata()),
                    );
                }
            }
        }
        let trailers = match stream.trailers().await {
            Ok(Some(t)) => metadata_json(&t),
            Ok(None) => Value::Object(Map::new()),
            Err(status) => {
                return (
                    head,
                    GrpcStatus::of(&status),
                    metadata_json(status.metadata()),
                );
            }
        };
        (
            head,
            GrpcStatus {
                code: 0,
                name: "OK".into(),
                message: String::new(),
            },
            trailers,
        )
    };
    let (head, status, trailers) = match channel.timeout {
        // A little longer than the deadline sent, so the server's answer is seen first.
        Some(t) => match tokio::time::timeout(t + Duration::from_millis(250), work).await {
            Ok(r) => r,
            Err(_) => (
                Value::Object(Map::new()),
                GrpcStatus {
                    code: 4,
                    name: "DEADLINE_EXCEEDED".into(),
                    message: format!("no answer within {} ms", t.as_millis()),
                },
                Value::Object(Map::new()),
            ),
        },
        None => work.await,
    };
    if headers.as_object().is_some_and(Map::is_empty) {
        headers = head;
    }
    Ok(CallOutcome {
        headers,
        messages,
        status,
        trailers,
        duration: started.elapsed(),
    })
}
