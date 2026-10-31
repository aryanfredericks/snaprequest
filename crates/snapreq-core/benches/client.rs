//! Engine overhead against a local server. Run with `cargo bench -p snapreq-core`.
//!
//! The numbers measure SnapRequest itself (request building, tracing, pooling), not the
//! network: the server is on loopback and answers immediately.
#![allow(clippy::unwrap_used, missing_docs)]

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use snapreq_core::model::{Body, HttpVersionPref, ProxyMode};
use snapreq_core::{Certificate, Client, ClientConfig, Request};
use tokio::net::TcpListener;
use tokio::runtime::Runtime;

async fn handler(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    // Drain the body so uploads are measured end to end.
    let len = req.into_body().collect().await.unwrap().to_bytes().len();
    Ok(hyper::Response::new(Full::new(Bytes::from(format!(
        "{{\"ok\":true,\"received\":{len}}}"
    )))))
}

/// HTTP/1.1 server, or HTTP/2 prior knowledge when `h2`.
async fn plain_server(h2: bool) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tcp.set_nodelay(true).unwrap();
            tokio::spawn(async move {
                let io = TokioIo::new(tcp);
                if h2 {
                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .serve_connection(io, service_fn(handler))
                        .await;
                } else {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service_fn(handler))
                        .await;
                }
            });
        }
    });
    addr
}

async fn tls_server() -> (SocketAddr, Certificate) {
    let params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()]).unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let mut config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .unwrap()
    .with_no_client_auth()
    .with_single_cert(
        vec![cert.der().clone()],
        PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
    )
    .unwrap();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tcp.set_nodelay(true).unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                if let Ok(tls) = acceptor.accept(tcp).await {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(TokioIo::new(tls), service_fn(handler))
                        .await;
                }
            });
        }
    });
    (addr, Certificate::from_der(cert.der().to_vec()))
}

fn get(url: &str) -> Request {
    let mut req = Request::get(url);
    req.settings.proxy = ProxyMode::Direct;
    req
}

fn benches(c: &mut Criterion) {
    let rt = Runtime::new().unwrap();
    let h1 = rt.block_on(plain_server(false));
    let h2 = rt.block_on(plain_server(true));
    let (tls, cert) = rt.block_on(tls_server());

    let client = Client::new(ClientConfig {
        extra_root_certificates: vec![cert],
        ..ClientConfig::default()
    })
    .unwrap();
    let no_pool = Client::new(ClientConfig {
        pool_max_idle_per_host: 0,
        ..ClientConfig::default()
    })
    .unwrap();

    let mut group = c.benchmark_group("http1");
    let url = format!("http://{h1}/");
    group.bench_function("get_keep_alive", |b| {
        b.to_async(&rt)
            .iter(|| async { client.execute(&get(&url)).await.unwrap() });
    });
    group.bench_function("get_new_connection", |b| {
        b.to_async(&rt)
            .iter(|| async { no_pool.execute(&get(&url)).await.unwrap() });
    });
    let mut upload = get(&url);
    upload.method = http::Method::POST;
    upload.body = Body::Text {
        content: "x".repeat(1024 * 1024),
        content_type: None,
    };
    group.throughput(Throughput::Bytes(1024 * 1024));
    group.bench_function("post_1mib", |b| {
        b.to_async(&rt)
            .iter(|| async { client.execute(&upload).await.unwrap() });
    });
    group.finish();

    let mut group = c.benchmark_group("https");
    let url = format!("https://{tls}/");
    group.bench_function("get_keep_alive", |b| {
        b.to_async(&rt)
            .iter(|| async { client.execute(&get(&url)).await.unwrap() });
    });
    group.finish();

    let mut group = c.benchmark_group("http2");
    let mut req = get(&format!("http://{h2}/"));
    req.settings.http_version = HttpVersionPref::Http2;
    group.throughput(Throughput::Elements(16));
    group.bench_function("16_concurrent_gets", |b| {
        b.to_async(&rt).iter(|| async {
            let all = (0..16).map(|_| client.execute(&req));
            for r in futures_util::future::join_all(all).await {
                r.unwrap();
            }
        });
    });
    group.finish();
}

criterion_group!(client, benches);
criterion_main!(client);
