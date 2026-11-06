//! Service descriptions: from proto files or server reflection, listed, and JSON for messages.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;

use prost::Message as _;
use prost_reflect::{
    DescriptorPool, DynamicMessage, FieldDescriptor, Kind, MessageDescriptor, MethodDescriptor,
    SerializeOptions,
};
use serde::Serialize;
use serde_json::{Value, json};

use crate::channel::Channel;
use crate::{Error, Result};

/// Compiles proto files (`includes` are import paths; each file's own directory is one too).
pub fn compile_protos(files: &[PathBuf], includes: &[PathBuf]) -> Result<DescriptorPool> {
    if files.is_empty() {
        return Err(Error::Proto("no proto files given".into()));
    }
    let mut paths: Vec<PathBuf> = includes.to_vec();
    for f in files {
        if let Some(dir) = f.parent()
            && !paths.iter().any(|p| p == dir)
        {
            paths.push(if dir.as_os_str().is_empty() {
                PathBuf::from(".")
            } else {
                dir.to_path_buf()
            });
        }
    }
    let set = protox::compile(files, &paths).map_err(|e| Error::Proto(e.to_string()))?;
    DescriptorPool::from_file_descriptor_set(set).map_err(|e| Error::Proto(e.to_string()))
}

/// One service and its methods.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceInfo {
    pub name: String,
    pub methods: Vec<MethodInfo>,
}

/// One method, with a JSON template of its input.
#[derive(Debug, Clone, Serialize)]
pub struct MethodInfo {
    pub name: String,
    /// `package.Service/Method`.
    pub path: String,
    pub input: String,
    pub output: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
    pub template: Value,
}

impl MethodInfo {
    /// `unary`, `server streaming`, `client streaming` or `bidirectional streaming`.
    pub fn kind(&self) -> &'static str {
        match (self.client_streaming, self.server_streaming) {
            (false, false) => "unary",
            (false, true) => "server streaming",
            (true, false) => "client streaming",
            (true, true) => "bidirectional streaming",
        }
    }
}

/// Every service of `pool` (the reflection service itself left out), sorted by name.
pub fn describe(pool: &DescriptorPool) -> Vec<ServiceInfo> {
    let mut out: Vec<ServiceInfo> = pool
        .services()
        .filter(|s| !s.full_name().starts_with("grpc.reflection."))
        .map(|s| ServiceInfo {
            name: s.full_name().to_owned(),
            methods: s
                .methods()
                .map(|m| MethodInfo {
                    name: m.name().to_owned(),
                    path: format!("{}/{}", s.full_name(), m.name()),
                    input: m.input().full_name().to_owned(),
                    output: m.output().full_name().to_owned(),
                    client_streaming: m.is_client_streaming(),
                    server_streaming: m.is_server_streaming(),
                    template: message_template(&m.input()),
                })
                .collect(),
        })
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A method by `package.Service/Method` (or `package.Service.Method`).
pub fn find_method(pool: &DescriptorPool, name: &str) -> Result<MethodDescriptor> {
    let name = name.trim().trim_start_matches('/');
    let (service, method) = name
        .rsplit_once('/')
        .or_else(|| name.rsplit_once('.'))
        .ok_or_else(|| {
            Error::Invalid(format!(
                "`{name}` is not a method (expected package.Service/Method)"
            ))
        })?;
    let svc = pool.get_service_by_name(service).ok_or_else(|| {
        let known: Vec<String> = describe(pool).into_iter().map(|s| s.name).collect();
        Error::Invalid(format!(
            "no service `{service}` (services: {})",
            if known.is_empty() {
                "none".into()
            } else {
                known.join(", ")
            }
        ))
    })?;
    svc.methods().find(|m| m.name() == method).ok_or_else(|| {
        let known: Vec<String> = svc.methods().map(|m| m.name().to_owned()).collect();
        Error::Invalid(format!(
            "no method `{method}` in `{service}` (methods: {})",
            known.join(", ")
        ))
    })
}

/// A message from its proto3 JSON form.
pub fn message_from_json(desc: &MessageDescriptor, json: &str) -> Result<DynamicMessage> {
    let text = if json.trim().is_empty() { "{}" } else { json };
    let mut de = serde_json::Deserializer::from_str(text);
    let msg = DynamicMessage::deserialize(desc.clone(), &mut de)
        .map_err(|e| Error::Invalid(format!("not a valid {} message: {e}", desc.full_name())))?;
    de.end()
        .map_err(|e| Error::Invalid(format!("not a valid {} message: {e}", desc.full_name())))?;
    Ok(msg)
}

/// A message in proto3 JSON form, with fields at their default values shown.
pub fn message_to_json(msg: &DynamicMessage) -> Value {
    let options = SerializeOptions::new().skip_default_fields(false);
    msg.serialize_with_options(serde_json::value::Serializer, &options)
        .unwrap_or(Value::Null)
}

/// A JSON skeleton of a message: every field with a placeholder value (nested messages a few
/// levels deep; the first field of each `oneof`).
pub fn message_template(desc: &MessageDescriptor) -> Value {
    template(desc, 0)
}

fn template(desc: &MessageDescriptor, depth: usize) -> Value {
    let mut out = serde_json::Map::new();
    let mut oneofs = HashSet::new();
    for f in desc.fields() {
        if let Some(o) = f.containing_oneof()
            && !oneofs.insert(o.full_name().to_owned())
        {
            continue;
        }
        out.insert(f.json_name().to_owned(), field_template(&f, depth));
    }
    Value::Object(out)
}

fn field_template(f: &FieldDescriptor, depth: usize) -> Value {
    if f.is_map() {
        return json!({});
    }
    let one = match f.kind() {
        Kind::Message(m) if m.full_name() == "google.protobuf.Timestamp" => {
            json!("1970-01-01T00:00:00Z")
        }
        Kind::Message(m) if m.full_name() == "google.protobuf.Duration" => json!("0s"),
        Kind::Message(m) if depth < 3 => template(&m, depth + 1),
        Kind::Message(_) => json!({}),
        Kind::Enum(e) => json!(
            e.values()
                .next()
                .map(|v| v.name().to_owned())
                .unwrap_or_default()
        ),
        Kind::String => json!(""),
        Kind::Bytes => json!(""),
        Kind::Bool => json!(false),
        Kind::Double | Kind::Float => json!(0.0),
        Kind::Int64 | Kind::Uint64 | Kind::Sint64 | Kind::Fixed64 | Kind::Sfixed64 => json!("0"),
        _ => json!(0),
    };
    if f.is_list() { json!([one]) } else { one }
}

macro_rules! reflection_with {
    ($pb:path, $channel:expr) => {{
        use $pb as pb;
        use pb::server_reflection_client::ServerReflectionClient;
        use pb::server_reflection_request::MessageRequest;
        use pb::server_reflection_response::MessageResponse;

        let channel: &Channel = $channel;
        let mut client = ServerReflectionClient::with_origin(channel.send.clone(), channel.origin.clone());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<pb::ServerReflectionRequest>();
        let requests = futures_util::stream::unfold(rx, |mut rx| async move { rx.recv().await.map(|r| (r, rx)) });
        let mut request = tonic::Request::new(requests);
        crate::call::add_metadata(request.metadata_mut(), &channel.metadata)?;
        let ask = |m: MessageRequest| {
            tx.send(pb::ServerReflectionRequest { host: String::new(), message_request: Some(m) })
                .map_err(|_| Error::Reflection("the stream closed".into()))
        };
        ask(MessageRequest::ListServices(String::new()))?;
        let mut responses = client.server_reflection_info(request).await.map_err(Status)?.into_inner();
        let mut next = async || -> Result<MessageResponse> {
            let r = responses
                .message()
                .await
                .map_err(Status)?
                .ok_or_else(|| Error::Reflection("the server ended the stream".into()))?;
            match r.message_response {
                Some(MessageResponse::ErrorResponse(e)) => {
                    Err(Error::Reflection(format!("{} (code {})", e.error_message, e.error_code)))
                }
                Some(m) => Ok(m),
                None => Err(Error::Reflection("an empty answer".into())),
            }
        };
        let services = match next().await? {
            MessageResponse::ListServicesResponse(l) => l.service,
            _ => return Err(Error::Reflection("unexpected answer to list services".into())),
        };
        let mut files: HashMap<String, prost_types::FileDescriptorProto> = HashMap::new();
        let mut wanted: VecDeque<MessageRequest> = services
            .iter()
            .filter(|s| !s.name.starts_with("grpc.reflection."))
            .map(|s| MessageRequest::FileContainingSymbol(s.name.clone()))
            .collect();
        while let Some(req) = wanted.pop_front() {
            ask(req)?;
            if let MessageResponse::FileDescriptorResponse(fd) = next().await? {
                for bytes in fd.file_descriptor_proto {
                    let file = prost_types::FileDescriptorProto::decode(bytes.as_slice())
                        .map_err(|e| Error::Reflection(format!("bad file descriptor: {e}")))?;
                    for dep in &file.dependency {
                        if !files.contains_key(dep) && !wanted.iter().any(|w| matches!(w, MessageRequest::FileByFilename(f) if f == dep)) {
                            wanted.push_back(MessageRequest::FileByFilename(dep.clone()));
                        }
                    }
                    files.insert(file.name().to_owned(), file);
                }
            }
            wanted.retain(|w| !matches!(w, MessageRequest::FileByFilename(f) if files.contains_key(f)));
        }
        drop(tx);
        let mut pool = DescriptorPool::new();
        pool.add_file_descriptor_protos(files.into_values())
            .map_err(|e| Error::Reflection(e.to_string()))?;
        Ok::<DescriptorPool, Error>(pool)
    }};
}

#[allow(non_snake_case)]
fn Status(s: tonic::Status) -> Error {
    Error::Reflection(format!(
        "{}: {}",
        crate::status_name(s.code() as i32),
        s.message()
    ))
}

/// The services of the server, by reflection (`grpc.reflection.v1`, else `v1alpha`).
pub async fn reflect(channel: &Channel) -> Result<DescriptorPool> {
    match reflection_with!(tonic_reflection::pb::v1, channel) {
        Err(Error::Reflection(e)) if e.starts_with("UNIMPLEMENTED") => {
            reflection_with!(tonic_reflection::pb::v1alpha, channel).map_err(|e2| match e2 {
                Error::Reflection(m) if m.starts_with("UNIMPLEMENTED") => Error::Reflection(
                    "the server does not support reflection; give its proto files".into(),
                ),
                other => other,
            })
        }
        other => other,
    }
}
