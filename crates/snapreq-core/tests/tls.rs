//! HTTPS, host pinning and Happy Eyeballs against local servers. Certificates are
//! generated per test run, so no internet or fixture files are needed.
#![allow(clippy::unwrap_used)] // test-server helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use snapreq_core::model::{Phase, ResolveOverride, TraceEntry, TraceEvent};
use snapreq_core::{Certificate, Client, ClientConfig, Error, ExecutionError, Request};
use tokio::net::TcpListener;
use tokio_rustls::TlsAcceptor;

struct TlsServer {
    addr: SocketAddr,
    /// The server's self-signed certificate, to be trusted by the client.
    cert: Certificate,
}

/// Replies with the `Host` header it received.
async fn echo_host(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let host = req
        .headers()
        .get("host")
        .map(|h| h.to_str().unwrap().to_owned())
        .unwrap_or_default();
    Ok(hyper::Response::new(Full::new(Bytes::from(host))))
}

/// HTTPS server with a self-signed certificate for `snap.test` and `127.0.0.1`.
async fn start_tls_server() -> TlsServer {
    let mut params =
        rcgen::CertificateParams::new(vec!["snap.test".into(), "127.0.0.1".into()]).unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "SnapRequest Test");
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
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let acceptor = TlsAcceptor::from(Arc::new(config));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                // Handshake failures are expected in the negative tests.
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), service_fn(echo_host))
                    .await;
            });
        }
    });

    TlsServer {
        addr,
        cert: Certificate::from_der(cert.der().to_vec()),
    }
}

fn trusting(cert: &Certificate) -> Client {
    Client::new(ClientConfig {
        extra_root_certificates: vec![cert.clone()],
        ..ClientConfig::default()
    })
    .unwrap()
}

fn names(trace: &[TraceEntry]) -> Vec<String> {
    trace
        .iter()
        .map(|e| {
            let json = serde_json::to_value(e).unwrap();
            json["event"].as_str().unwrap().to_owned()
        })
        .collect()
}

fn execution_error(err: Error) -> ExecutionError {
    match err {
        Error::Execution(e) => *e,
        other => panic!("expected an execution error, got {other:?}"),
    }
}

fn pin(host: &str, addrs: &[&str]) -> ResolveOverride {
    ResolveOverride {
        host: host.into(),
        port: None,
        addrs: addrs.iter().map(|a| a.parse().unwrap()).collect(),
    }
}

#[tokio::test]
async fn https_request_records_the_handshake() {
    let server = start_tls_server().await;
    let client = trusting(&server.cert);

    let resp = client
        .execute(&Request::get(format!("https://{}/", server.addr)))
        .await
        .unwrap();

    assert_eq!(resp.status, 200);
    assert_eq!(
        names(&resp.trace),
        [
            "connect_start",
            "connect_end",
            "tls_start",
            "tls_end",
            "request_headers",
            "request_sent",
            "response_first_byte",
            "response_headers",
            "response_complete",
        ]
    );
    let info = resp
        .trace
        .iter()
        .find_map(|e| match &e.event {
            TraceEvent::TlsEnd { info, .. } => Some(info.clone()),
            _ => None,
        })
        .unwrap();
    assert_eq!(info.version, "TLSv1.3");
    assert!(info.cipher.starts_with("TLS13_"), "{}", info.cipher);
    assert_eq!(info.alpn.as_deref(), Some("http/1.1"));
    assert_eq!(info.peer_certificates.len(), 1);
    assert_eq!(info.peer_certificates[0].subject, "CN=SnapRequest Test");
    assert_eq!(
        info.peer_certificates[0].subject_alt_names,
        ["snap.test", "127.0.0.1"]
    );
    assert!(resp.timings.tls.is_some());
}

#[tokio::test]
async fn untrusted_certificate_fails_in_the_tls_phase() {
    let server = start_tls_server().await;
    let err = execution_error(
        Client::default()
            .execute(&Request::get(format!("https://{}/", server.addr)))
            .await
            .unwrap_err(),
    );
    assert_eq!(err.phase, Phase::Tls);
    assert!(err.message.contains("UnknownIssuer"), "{}", err.message);
    assert_eq!(
        names(&err.trace),
        ["connect_start", "connect_end", "tls_start", "error"]
    );
}

#[tokio::test]
async fn pinned_host_skips_dns_and_verifies_the_url_host() {
    let server = start_tls_server().await;
    let client = trusting(&server.cert);
    let mut req = Request::get(format!("https://snap.test:{}/", server.addr.port()));
    req.settings.resolve = vec![pin("snap.test", &["127.0.0.1"])];

    let resp = client.execute(&req).await.unwrap();

    assert_eq!(
        resp.text(),
        Some(format!("snap.test:{}", server.addr.port()).as_str())
    );
    let names = names(&resp.trace);
    assert_eq!(names[0], "info");
    assert!(!names.contains(&"dns_start".to_owned()));
    assert!(resp.timings.dns.is_none());
}

#[tokio::test]
async fn certificate_for_another_name_is_rejected() {
    let server = start_tls_server().await;
    let client = trusting(&server.cert);
    let mut req = Request::get(format!("https://other.test:{}/", server.addr.port()));
    req.settings.resolve = vec![pin("other.test", &["127.0.0.1"])];

    let err = execution_error(client.execute(&req).await.unwrap_err());
    assert_eq!(err.phase, Phase::Tls);
    assert!(
        err.message.contains("not valid for name \"other.test\""),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn connections_are_reused_without_a_new_handshake() {
    let server = start_tls_server().await;
    let client = trusting(&server.cert);
    let url = format!("https://{}/", server.addr);

    client.execute(&Request::get(&url)).await.unwrap();
    let second = client.execute(&Request::get(&url)).await.unwrap();

    let names = names(&second.trace);
    assert_eq!(names[0], "connection_reused");
    assert!(!names.contains(&"tls_start".to_owned()));
    assert!(second.timings.tls.is_none());
}

#[tokio::test]
async fn falls_back_to_the_next_address_when_one_refuses() {
    // Something listening on 127.0.0.1 only; nothing on [::1] at that port.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tcp), service_fn(echo_host))
                    .await;
            });
        }
    });

    let mut req = Request::get(format!("http://dual.test:{port}/"));
    // IPv6 first, as resolvers usually return it.
    req.settings.resolve = vec![pin("dual.test", &["::1", "127.0.0.1"])];
    let resp = Client::default().execute(&req).await.unwrap();

    assert_eq!(resp.status, 200);
    assert_eq!(resp.remote_addr.unwrap().ip().to_string(), "127.0.0.1");
    let starts = names(&resp.trace)
        .iter()
        .filter(|n| *n == "connect_start")
        .count();
    assert_eq!(starts, 2);
}

#[tokio::test]
async fn invalid_extra_root_is_rejected() {
    let err = Client::new(ClientConfig {
        extra_root_certificates: vec![Certificate::from_der(b"junk".to_vec())],
        ..ClientConfig::default()
    })
    .unwrap_err();
    assert!(
        err.to_string().contains("invalid root certificate"),
        "{err}"
    );
}
