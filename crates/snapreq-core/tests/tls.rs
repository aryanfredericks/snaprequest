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
use snapreq_core::model::{Phase, ResolveOverride, TlsVersion, TraceEntry, TraceEvent};
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

/// A self-signed certificate and its key.
fn self_signed(common_name: &str, names: &[&str]) -> (rcgen::Certificate, rcgen::KeyPair) {
    let mut params =
        rcgen::CertificateParams::new(names.iter().map(|n| (*n).to_owned()).collect::<Vec<_>>())
            .unwrap();
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, common_name);
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    (cert, key)
}

#[derive(Default)]
struct ServerOptions {
    /// Require a client certificate issued by (or equal to) this certificate.
    client_ca: Option<rustls::pki_types::CertificateDer<'static>>,
    /// Only offer TLS 1.2.
    tls12_only: bool,
}

/// HTTPS server with a self-signed certificate for `snap.test` and `127.0.0.1`.
async fn start_tls_server() -> TlsServer {
    start_tls_server_with(ServerOptions::default()).await
}

async fn start_tls_server_with(opts: ServerOptions) -> TlsServer {
    let (cert, key) = self_signed("SnapRequest Test", &["snap.test", "127.0.0.1"]);

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let versions: &[&rustls::SupportedProtocolVersion] = if opts.tls12_only {
        &[&rustls::version::TLS12]
    } else {
        rustls::DEFAULT_VERSIONS
    };
    let builder = rustls::ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(versions)
        .unwrap();
    let builder = match opts.client_ca {
        None => builder.with_no_client_auth(),
        Some(ca) => {
            let mut roots = rustls::RootCertStore::empty();
            roots.add(ca).unwrap();
            let verifier = rustls::server::WebPkiClientVerifier::builder_with_provider(
                Arc::new(roots),
                provider,
            )
            .build()
            .unwrap();
            builder.with_client_cert_verifier(verifier)
        }
    };
    let mut config = builder
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

fn temp_file(name: &str, contents: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("snapreq-tls-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, contents).unwrap();
    path
}

fn infos(trace: &[TraceEntry]) -> Vec<String> {
    trace
        .iter()
        .filter_map(|e| match &e.event {
            TraceEvent::Info { message } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

fn tls_end_info(trace: &[TraceEntry]) -> snapreq_core::model::TlsInfo {
    trace
        .iter()
        .find_map(|e| match &e.event {
            TraceEvent::TlsEnd { info, .. } => Some(info.clone()),
            _ => None,
        })
        .unwrap()
}

#[tokio::test]
async fn verification_off_connects_and_says_why_it_would_have_failed() {
    let server = start_tls_server().await;
    let mut req = Request::get(format!("https://{}/", server.addr));
    req.settings.tls.verify = false;

    let resp = Client::default().execute(&req).await.unwrap();

    assert_eq!(resp.status, 200);
    let infos = infos(&resp.trace);
    assert_eq!(infos.len(), 1, "{infos:?}");
    assert!(
        infos[0].contains("verification is DISABLED"),
        "{}",
        infos[0]
    );
    assert!(infos[0].contains("UnknownIssuer"), "{}", infos[0]);
}

#[tokio::test]
async fn verification_off_with_a_trusted_certificate_says_it_would_have_passed() {
    let server = start_tls_server().await;
    let mut req = Request::get(format!("https://{}/", server.addr));
    req.settings.tls.verify = false;

    let resp = trusting(&server.cert).execute(&req).await.unwrap();
    assert!(infos(&resp.trace)[0].contains("would have passed"));
}

#[tokio::test]
async fn insecure_connections_are_never_reused_by_verifying_requests() {
    let server = start_tls_server().await;
    let client = Client::default();
    let url = format!("https://{}/", server.addr);

    let mut insecure = Request::get(&url);
    insecure.settings.tls.verify = false;
    client.execute(&insecure).await.unwrap();

    // Same client, same host, verification on: must do a fresh handshake and fail.
    let err = execution_error(client.execute(&Request::get(&url)).await.unwrap_err());
    assert_eq!(err.phase, Phase::Tls);
    assert!(!names(&err.trace).contains(&"connection_reused".to_owned()));

    // An identical insecure request may reuse the insecure connection.
    let again = client.execute(&insecure).await.unwrap();
    assert_eq!(names(&again.trace)[0], "connection_reused");
}

#[tokio::test]
async fn ca_certificate_file_per_request() {
    let server = start_tls_server().await;
    let pem = format!(
        "-----BEGIN CERTIFICATE-----\n{}\n-----END CERTIFICATE-----\n",
        base64_lines(server.cert.as_der())
    );
    let mut req = Request::get(format!("https://{}/", server.addr));
    req.settings.tls.ca_cert = Some(temp_file("server-ca.pem", pem.as_bytes()));

    let resp = Client::default().execute(&req).await.unwrap();
    assert_eq!(resp.status, 200);
    assert!(infos(&resp.trace).is_empty());
}

fn base64_lines(der: &[u8]) -> String {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(der);
    b64.as_bytes()
        .chunks(64)
        .map(|c| std::str::from_utf8(c).unwrap())
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn mutual_tls() {
    let (client_cert, client_key) = self_signed("snap-client", &["client.snap.test"]);
    let server = start_tls_server_with(ServerOptions {
        client_ca: Some(client_cert.der().clone()),
        ..ServerOptions::default()
    })
    .await;
    let client = trusting(&server.cert);
    let url = format!("https://{}/", server.addr);

    // Without a client certificate the server refuses. With TLS 1.3 the refusal can
    // arrive just after the handshake, so only check that it fails.
    assert!(client.execute(&Request::get(&url)).await.is_err());

    let mut req = Request::get(&url);
    req.settings.tls.client_cert = Some(snapreq_core::model::ClientCertificate {
        cert: temp_file("client.crt", client_cert.pem().as_bytes()),
        key: temp_file("client.key", client_key.serialize_pem().as_bytes()),
    });
    let resp = client.execute(&req).await.unwrap();

    assert_eq!(resp.status, 200);
    assert!(
        infos(&resp.trace).contains(&"presenting client certificate CN=snap-client".to_owned()),
        "{:?}",
        infos(&resp.trace)
    );
}

#[tokio::test]
async fn tls_version_limits() {
    let server = start_tls_server_with(ServerOptions {
        tls12_only: true,
        ..ServerOptions::default()
    })
    .await;
    let client = trusting(&server.cert);
    let url = format!("https://{}/", server.addr);

    let resp = client.execute(&Request::get(&url)).await.unwrap();
    assert_eq!(tls_end_info(&resp.trace).version, "TLSv1.2");

    let mut tls13_only = Request::get(&url);
    tls13_only.settings.tls.min_version = Some(TlsVersion::Tls13);
    let err = execution_error(client.execute(&tls13_only).await.unwrap_err());
    assert_eq!(err.phase, Phase::Tls);
}
