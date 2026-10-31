//! HTTP/2 over TLS (ALPN) and over plain TCP (prior knowledge), against local servers.
#![allow(clippy::unwrap_used)] // test-server helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use http_body_util::{BodyExt, Full};
use hyper::service::service_fn;
use hyper_util::rt::{TokioExecutor, TokioIo};
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use snapreq_core::model::{Body, HttpVersionPref, Phase, TraceEvent};
use snapreq_core::{Certificate, Client, ClientConfig, Error, Request};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

/// Replies `<version> <method> <authority><path> <body length>`.
async fn describe(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let version = format!("{:?}", req.version());
    let method = req.method().to_string();
    let authority = req
        .uri()
        .authority()
        .map(|a| a.to_string())
        .or_else(|| {
            req.headers()
                .get("host")
                .map(|h| h.to_str().unwrap().to_owned())
        })
        .unwrap_or_default();
    let path = req.uri().path().to_owned();
    let body = req.into_body().collect().await.unwrap().to_bytes();
    Ok(hyper::Response::new(Full::new(Bytes::from(format!(
        "{version} {method} {authority}{path} {}",
        body.len()
    )))))
}

struct Server {
    addr: SocketAddr,
    cert: Option<Certificate>,
    connections: Arc<AtomicUsize>,
}

/// TLS server; speaks HTTP/2 when ALPN selects `h2` (if `offer_h2`), else HTTP/1.1.
async fn tls_server(offer_h2: bool) -> Server {
    let mut params = rcgen::CertificateParams::new(vec!["127.0.0.1".to_owned()]).unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "h2 test");
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .unwrap()
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(key.serialize_der())),
        )
        .unwrap();
    config.alpn_protocols = if offer_h2 {
        vec![b"h2".to_vec(), b"http/1.1".to_vec()]
    } else {
        vec![b"http/1.1".to_vec()]
    };
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = connections.clone();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let h2 = tls.get_ref().1.alpn_protocol() == Some(b"h2");
                let io = TokioIo::new(tls);
                if h2 {
                    let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                        .serve_connection(io, service_fn(describe))
                        .await;
                } else {
                    let _ = hyper::server::conn::http1::Builder::new()
                        .serve_connection(io, service_fn(describe))
                        .await;
                }
            });
        }
    });

    Server {
        addr,
        cert: Some(Certificate::from_der(cert.der().to_vec())),
        connections,
    }
}

/// Plain-TCP HTTP/2 server (prior knowledge, no TLS).
async fn h2c_server() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let connections = Arc::new(AtomicUsize::new(0));
    let counter = connections.clone();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                let _ = hyper::server::conn::http2::Builder::new(TokioExecutor::new())
                    .serve_connection(TokioIo::new(tcp), service_fn(describe))
                    .await;
            });
        }
    });
    Server {
        addr,
        cert: None,
        connections,
    }
}

fn client_for(server: &Server) -> Client {
    Client::new(ClientConfig {
        extra_root_certificates: server.cert.iter().cloned().collect(),
        ..ClientConfig::default()
    })
    .unwrap()
}

fn alpn(resp: &snapreq_core::Response) -> Option<String> {
    resp.trace.iter().find_map(|e| match &e.event {
        TraceEvent::TlsEnd { info, .. } => info.alpn.clone(),
        _ => None,
    })
}

#[tokio::test]
async fn auto_negotiates_http2_over_tls() {
    let server = tls_server(true).await;
    let req =
        Request::new(http::Method::POST, format!("https://{}/h2", server.addr)).body(Body::Text {
            content: "hello".into(),
            content_type: None,
        });
    let resp = client_for(&server).execute(&req).await.unwrap();

    assert_eq!(resp.version.as_str(), "HTTP/2");
    assert_eq!(alpn(&resp).as_deref(), Some("h2"));
    assert_eq!(
        resp.text().unwrap(),
        format!("HTTP/2.0 POST {}/h2 5", server.addr)
    );

    // Request headers in the trace use HTTP/2 form: lowercase, no Host.
    let sent = resp
        .trace
        .iter()
        .find_map(|e| match &e.event {
            TraceEvent::RequestHeaders {
                headers, version, ..
            } => Some((version.as_str(), headers.clone())),
            _ => None,
        })
        .unwrap();
    assert_eq!(sent.0, "HTTP/2");
    assert!(sent.1.iter().all(|h| h.name == h.name.to_lowercase()));
    assert!(!sent.1.iter().any(|h| h.name == "host"));
    // Timings are still complete.
    let t = &resp.timings;
    assert!(t.tls.is_some() && t.send.is_some() && t.wait.is_some() && t.download.is_some());
}

#[tokio::test]
async fn auto_falls_back_to_http1_when_the_server_lacks_h2() {
    let server = tls_server(false).await;
    let resp = client_for(&server)
        .execute(&Request::get(format!("https://{}/", server.addr)))
        .await
        .unwrap();
    assert_eq!(resp.version.as_str(), "HTTP/1.1");
    assert_eq!(alpn(&resp).as_deref(), Some("http/1.1"));
}

#[tokio::test]
async fn http1_preference_is_honoured_even_if_h2_is_available() {
    let server = tls_server(true).await;
    let mut req = Request::get(format!("https://{}/", server.addr));
    req.settings.http_version = HttpVersionPref::Http1;
    let resp = client_for(&server).execute(&req).await.unwrap();
    assert_eq!(resp.version.as_str(), "HTTP/1.1");
}

#[tokio::test]
async fn http2_only_fails_clearly_without_server_support() {
    let server = tls_server(false).await;
    let mut req = Request::get(format!("https://{}/", server.addr));
    req.settings.http_version = HttpVersionPref::Http2;
    let err = client_for(&server).execute(&req).await.unwrap_err();
    let Error::Execution(err) = err else {
        panic!("expected execution error, got {err:?}");
    };
    assert_eq!(err.phase, Phase::Tls);
}

#[tokio::test]
async fn prior_knowledge_http2_over_plain_tcp() {
    let server = h2c_server().await;
    let mut req = Request::get(format!("http://{}/plain", server.addr));
    req.settings.http_version = HttpVersionPref::Http2;
    let resp = client_for(&server).execute(&req).await.unwrap();

    assert_eq!(resp.version.as_str(), "HTTP/2");
    assert_eq!(
        resp.text().unwrap(),
        format!("HTTP/2.0 GET {}/plain 0", server.addr)
    );
}

#[tokio::test]
async fn one_http2_connection_serves_concurrent_requests() {
    let server = tls_server(true).await;
    let client = client_for(&server);
    let url = format!("https://{}/", server.addr);

    client.execute(&Request::get(&url)).await.unwrap();
    let requests: Vec<_> = (0..8)
        .map(|_| {
            let client = client.clone();
            let url = url.clone();
            tokio::spawn(async move { client.execute(&Request::get(url)).await })
        })
        .collect();
    for r in requests {
        let resp = r.await.unwrap().unwrap();
        assert_eq!(resp.version.as_str(), "HTTP/2");
        assert!(matches!(
            resp.trace[0].event,
            TraceEvent::ConnectionReused { .. }
        ));
    }
    assert_eq!(server.connections.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn http2_reports_body_sizes_but_not_header_sizes() {
    let server = h2c_server().await;
    let mut req =
        Request::new(http::Method::PUT, format!("http://{}/", server.addr)).body(Body::Text {
            content: "x".repeat(1000),
            content_type: None,
        });
    req.settings.http_version = HttpVersionPref::Http2;
    let resp = client_for(&server).execute(&req).await.unwrap();

    assert_eq!(resp.sizes.request_body, 1000);
    assert_eq!(resp.sizes.request_headers, 0);
    assert_eq!(resp.sizes.response_headers, 0);
    assert!(resp.text().unwrap().ends_with(" 1000"));
}

#[tokio::test]
async fn wire_capture_explains_it_is_unavailable_for_http2() {
    let server = h2c_server().await;
    let mut req = Request::get(format!("http://{}/", server.addr));
    req.settings.http_version = HttpVersionPref::Http2;
    req.settings.capture_wire = true;
    let resp = client_for(&server).execute(&req).await.unwrap();

    assert!(resp.trace.iter().any(|e| matches!(
        &e.event,
        TraceEvent::Info { message } if message == "wire capture is not available for HTTP/2"
    )));
    assert!(
        !resp
            .trace
            .iter()
            .any(|e| matches!(e.event, TraceEvent::Wire { .. }))
    );
}
