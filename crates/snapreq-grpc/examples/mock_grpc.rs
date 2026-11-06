//! A mock gRPC server for tests and the web UI's end-to-end tests: the `shop.v1.Orders` service of
//! `tests/fixtures/shop.proto`, with server reflection (v1).
//!
//! ```sh
//! cargo run -p snapreq-grpc --example mock_grpc -- 8766
//! ```
//!
//! - `Get(GetOrder)`: `{id, item: "book for <x-user metadata or anonymous>", status: READY, tags:
//!   ["auth:<authorization metadata>"]}`; id 404 → NOT_FOUND
//! - `Watch(GetOrder)`: three orders 20 ms apart, PENDING, PENDING, READY
//! - `Upload(stream Order)`: `{count}`
//! - `Chat(stream Order)`: each order back, item in upper case
#![allow(clippy::unwrap_used, dead_code)] // example code, also included by tests

use std::convert::Infallible;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use futures_util::{Stream, StreamExt};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use prost_reflect::{DescriptorPool, DynamicMessage, MethodDescriptor, Value};
use snapreq_grpc::DynamicCodec;
use tokio::net::TcpListener;
use tonic::server::{Grpc, StreamingService};
use tonic::{Request, Response, Status, Streaming};

// Valid from every crate under `crates/` (tests elsewhere include this file).
pub const PROTO: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../snapreq-grpc/tests/fixtures/shop.proto"
);

/// The descriptors of `shop.proto`.
pub fn pool() -> DescriptorPool {
    snapreq_grpc::compile_protos(&[PROTO.into()], &[]).unwrap()
}

type Out = Pin<Box<dyn Stream<Item = Result<DynamicMessage, Status>> + Send>>;

#[derive(Clone)]
struct Shop {
    method: MethodDescriptor,
}

fn order(
    method: &MethodDescriptor,
    id: i64,
    item: &str,
    status: i32,
    tags: Vec<String>,
) -> DynamicMessage {
    let mut m = DynamicMessage::new(method.output());
    m.set_field_by_name("id", Value::I64(id));
    m.set_field_by_name("item", Value::String(item.into()));
    m.set_field_by_name("status", Value::EnumNumber(status));
    m.set_field_by_name(
        "tags",
        Value::List(tags.into_iter().map(Value::String).collect()),
    );
    m
}

impl StreamingService<DynamicMessage> for Shop {
    type Response = DynamicMessage;
    type ResponseStream = Out;
    type Future = Pin<Box<dyn Future<Output = Result<Response<Out>, Status>> + Send>>;

    fn call(&mut self, request: Request<Streaming<DynamicMessage>>) -> Self::Future {
        let method = self.method.clone();
        Box::pin(async move {
            let text = |name: &str| {
                request
                    .metadata()
                    .get(name)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned)
            };
            let user = text("x-user").unwrap_or_else(|| "anonymous".into());
            let auth = text("authorization").unwrap_or_else(|| "none".into());
            let mut inputs = request.into_inner();
            let out: Out = match method.name() {
                "Get" | "Watch" => {
                    let first = inputs
                        .message()
                        .await?
                        .ok_or_else(|| Status::invalid_argument("no message"))?;
                    let id = first
                        .get_field_by_name("id")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0);
                    if id == 404 {
                        return Err(Status::not_found("order 404 does not exist"));
                    }
                    if method.name() == "Get" {
                        let o = order(
                            &method,
                            id,
                            &format!("book for {user}"),
                            1,
                            vec![format!("auth:{auth}")],
                        );
                        Box::pin(futures_util::stream::iter([Ok(o)]))
                    } else {
                        let m = method.clone();
                        Box::pin(futures_util::stream::unfold(0, move |i| {
                            let m = m.clone();
                            async move {
                                if i == 3 {
                                    return None;
                                }
                                tokio::time::sleep(Duration::from_millis(20)).await;
                                Some((Ok(order(&m, id, "book", i32::from(i == 2), vec![])), i + 1))
                            }
                        }))
                    }
                }
                "Upload" => {
                    let mut count = 0;
                    while inputs.message().await?.is_some() {
                        count += 1;
                    }
                    let mut s = DynamicMessage::new(method.output());
                    s.set_field_by_name("count", Value::I32(count));
                    Box::pin(futures_util::stream::iter([Ok(s)]))
                }
                _ => Box::pin(inputs.map(|m| {
                    m.map(|mut o| {
                        let item = o
                            .get_field_by_name("item")
                            .and_then(|v| v.as_str().map(str::to_uppercase))
                            .unwrap_or_default();
                        o.set_field_by_name("item", Value::String(item));
                        o
                    })
                })),
            };
            let mut resp = Response::new(out);
            resp.metadata_mut()
                .insert("x-served-by", "mock-grpc".parse().unwrap());
            Ok(resp)
        })
    }
}

/// Serves on `listener` until the process ends.
pub async fn serve(listener: TcpListener) {
    let pool = pool();
    let reflection = tonic_reflection::server::Builder::configure()
        .register_encoded_file_descriptor_set(Box::leak(pool.encode_to_vec().into_boxed_slice()))
        .build_v1()
        .unwrap();
    loop {
        let (tcp, _) = listener.accept().await.unwrap();
        let pool = pool.clone();
        let reflection = reflection.clone();
        tokio::spawn(async move {
            let svc = service_fn(move |req: hyper::Request<hyper::body::Incoming>| {
                let pool = pool.clone();
                let mut reflection = reflection.clone();
                async move {
                    let path = req.uri().path().to_owned();
                    if path.starts_with("/grpc.reflection.v1.") {
                        return Ok::<_, Infallible>(
                            tower_service::Service::call(&mut reflection, req)
                                .await
                                .unwrap(),
                        );
                    }
                    let method = path
                        .trim_start_matches('/')
                        .split_once('/')
                        .and_then(|(s, m)| {
                            pool.get_service_by_name(s)?
                                .methods()
                                .find(|x| x.name() == m)
                        });
                    let Some(method) = method else {
                        return Ok(Status::unimplemented(format!("no method {path}")).into_http());
                    };
                    let mut grpc = Grpc::new(DynamicCodec::new(method.input()));
                    Ok(grpc.streaming(Shop { method }, req).await)
                }
            });
            let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                .serve_connection(TokioIo::new(tcp), svc)
                .await;
        });
    }
}

#[tokio::main]
async fn main() {
    let port: u16 = std::env::args()
        .nth(1)
        .and_then(|p| p.parse().ok())
        .unwrap_or(8766);
    let listener = TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    println!("mock gRPC server listening on 127.0.0.1:{port}");
    serve(listener).await;
}
