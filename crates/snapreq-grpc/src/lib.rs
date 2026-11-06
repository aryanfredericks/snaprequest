//! SnapRequest gRPC (PLAN P10.4, ADR-036): calls described at run time, from proto files
//! (compiled with `protox`) or server reflection, with JSON messages (`prost-reflect`).
//!
//! Connections go through the SnapRequest engine ([`snapreq_core::Client::open_h2_stream`]): DNS,
//! the outbound policy, proxies, TLS settings and the trace work as for HTTP requests. Calls of
//! every kind (unary, server, client and bidirectional streaming) use one streaming code path,
//! which is the same on the wire.

mod call;
mod channel;
mod codec;
mod describe;
mod error;

pub use call::{CallEvent, CallOutcome, GrpcStatus, call, metadata_json, status_name};
pub use channel::{Channel, connect, http_url};
pub use codec::DynamicCodec;
pub use describe::{
    MethodInfo, ServiceInfo, compile_protos, describe, find_method, message_from_json,
    message_template, message_to_json, reflect,
};
pub use error::{Error, Result};
pub use prost_reflect::{DescriptorPool, DynamicMessage, MethodDescriptor};
