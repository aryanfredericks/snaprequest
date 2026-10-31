//! Proxies: HTTP forward proxy, HTTP CONNECT tunnel (with TLS inside) and SOCKS5,
//! using small proxies written for the tests.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use rustls::pki_types::{PrivateKeyDer, PrivatePkcs8KeyDer};
use snapreq_core::model::{Phase, ProxyMode, TraceEntry, TraceEvent};
use snapreq_core::{Certificate, Client, ClientConfig, Error, Request};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;

/// Replies with the Host header it received.
async fn echo_host(
    req: hyper::Request<hyper::body::Incoming>,
) -> Result<hyper::Response<Full<Bytes>>, Infallible> {
    let host = req
        .headers()
        .get("host")
        .map(|h| h.to_str().unwrap().to_owned())
        .unwrap_or_default();
    Ok(hyper::Response::new(Full::new(Bytes::from(format!(
        "host={host}"
    )))))
}

async fn plain_target() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
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
    addr
}

/// HTTPS target with a certificate for `snap.test`.
async fn tls_target() -> (SocketAddr, Certificate) {
    let params = rcgen::CertificateParams::new(vec!["snap.test".to_owned()]).unwrap();
    let key = rcgen::KeyPair::generate().unwrap();
    let cert = params.self_signed(&key).unwrap();
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
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
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (tcp, _) = listener.accept().await.unwrap();
            let acceptor = acceptor.clone();
            tokio::spawn(async move {
                let Ok(tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(tls), service_fn(echo_host))
                    .await;
            });
        }
    });
    (addr, Certificate::from_der(cert.der().to_vec()))
}

async fn read_head(stream: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    // Byte by byte so nothing after the head is consumed.
    while !buf.ends_with(b"\r\n\r\n") {
        if stream.read(&mut byte).await.unwrap() == 0 {
            break;
        }
        buf.push(byte[0]);
    }
    String::from_utf8(buf).unwrap()
}

/// Every name the fake proxies are asked to reach goes to 127.0.0.1.
fn local(authority: &str) -> String {
    let port = authority.rsplit_once(':').unwrap().1;
    format!("127.0.0.1:{port}")
}

/// HTTP proxy. Plain requests are answered by the proxy itself (showing what it
/// received); CONNECT opens a tunnel. `required_auth` makes it demand credentials.
async fn http_proxy(required_auth: Option<&'static str>) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let (mut client, _) = listener.accept().await.unwrap();
            let log = log.clone();
            tokio::spawn(async move {
                let head = read_head(&mut client).await;
                let request_line = head.lines().next().unwrap().to_owned();
                log.lock().unwrap().push(head.clone());
                let auth_ok = required_auth.is_none_or(|expected| {
                    head.lines().any(|l| {
                        l.eq_ignore_ascii_case(&format!("proxy-authorization: {expected}"))
                    })
                });
                if !auth_ok {
                    let _ = client
                        .write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\n\r\n")
                        .await;
                    return;
                }
                if let Some(authority) = request_line
                    .strip_prefix("CONNECT ")
                    .and_then(|r| r.split_whitespace().next())
                {
                    let mut upstream = TcpStream::connect(local(authority)).await.unwrap();
                    client
                        .write_all(b"HTTP/1.1 200 Connection established\r\n\r\n")
                        .await
                        .unwrap();
                    let _ = tokio::io::copy_bidirectional(&mut client, &mut upstream).await;
                } else {
                    let body = format!("proxied: {request_line}");
                    let _ = client
                        .write_all(
                            format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{body}",
                                body.len()
                            )
                            .as_bytes(),
                        )
                        .await;
                }
            });
        }
    });
    (addr, seen)
}

/// SOCKS5 proxy. Records the requested target (`domain:port` or `ip:port`).
async fn socks5_proxy(
    credentials: Option<(&'static str, &'static str)>,
) -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        loop {
            let (mut c, _) = listener.accept().await.unwrap();
            let log = log.clone();
            tokio::spawn(async move {
                let mut hdr = [0u8; 2];
                c.read_exact(&mut hdr).await.unwrap();
                let mut methods = vec![0u8; usize::from(hdr[1])];
                c.read_exact(&mut methods).await.unwrap();
                match credentials {
                    None => c.write_all(&[5, 0]).await.unwrap(),
                    Some((user, pass)) => {
                        if !methods.contains(&2) {
                            c.write_all(&[5, 0xFF]).await.unwrap();
                            return;
                        }
                        c.write_all(&[5, 2]).await.unwrap();
                        let mut ver_len = [0u8; 2];
                        c.read_exact(&mut ver_len).await.unwrap();
                        let mut u = vec![0u8; usize::from(ver_len[1])];
                        c.read_exact(&mut u).await.unwrap();
                        let mut plen = [0u8; 1];
                        c.read_exact(&mut plen).await.unwrap();
                        let mut p = vec![0u8; usize::from(plen[0])];
                        c.read_exact(&mut p).await.unwrap();
                        let ok = u == user.as_bytes() && p == pass.as_bytes();
                        c.write_all(&[1, if ok { 0 } else { 1 }]).await.unwrap();
                        if !ok {
                            return;
                        }
                    }
                }
                let mut req = [0u8; 4];
                c.read_exact(&mut req).await.unwrap();
                let host = match req[3] {
                    1 => {
                        let mut ip = [0u8; 4];
                        c.read_exact(&mut ip).await.unwrap();
                        std::net::Ipv4Addr::from(ip).to_string()
                    }
                    3 => {
                        let mut len = [0u8; 1];
                        c.read_exact(&mut len).await.unwrap();
                        let mut name = vec![0u8; usize::from(len[0])];
                        c.read_exact(&mut name).await.unwrap();
                        String::from_utf8(name).unwrap()
                    }
                    4 => {
                        let mut ip = [0u8; 16];
                        c.read_exact(&mut ip).await.unwrap();
                        std::net::Ipv6Addr::from(ip).to_string()
                    }
                    _ => return,
                };
                let mut port = [0u8; 2];
                c.read_exact(&mut port).await.unwrap();
                let port = u16::from_be_bytes(port);
                log.lock().unwrap().push(format!("{host}:{port}"));
                let mut upstream = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
                c.write_all(&[5, 0, 0, 1, 127, 0, 0, 1, 0, 0])
                    .await
                    .unwrap();
                let _ = tokio::io::copy_bidirectional(&mut c, &mut upstream).await;
            });
        }
    });
    (addr, seen)
}

fn via(url: String, proxy: String) -> Request {
    let mut req = Request::get(url);
    req.settings.proxy = ProxyMode::Url(proxy);
    req
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

fn has_dns(trace: &[TraceEntry]) -> bool {
    trace
        .iter()
        .any(|e| matches!(e.event, TraceEvent::DnsStart { .. }))
}

#[tokio::test]
async fn plain_http_goes_to_the_proxy_in_absolute_form_with_credentials() {
    let (proxy, seen) = http_proxy(Some("Basic dXNlcjpzZWNyZXQ=")).await;
    let req = via(
        "http://api.example.test/items?id=1".into(),
        format!("http://user:secret@{proxy}"),
    );
    let resp = Client::default().execute(&req).await.unwrap();

    assert_eq!(
        resp.text(),
        Some("proxied: GET http://api.example.test/items?id=1 HTTP/1.1")
    );
    assert_eq!(resp.remote_addr, Some(proxy));
    // The target name is never resolved locally.
    assert!(!has_dns(&resp.trace));
    assert!(infos(&resp.trace)[0].starts_with(&format!("using proxy http://user:***@{proxy}")));
    // The credential is redacted in the trace but sent on the wire.
    let traced = format!("{:?}", resp.trace);
    assert!(traced.contains("Basic <redacted>") && !traced.contains("dXNlcjpzZWNyZXQ="));
    assert!(seen.lock().unwrap()[0].contains("Proxy-Authorization: Basic dXNlcjpzZWNyZXQ="));
}

#[tokio::test]
async fn https_goes_through_a_connect_tunnel() {
    let (proxy, seen) = http_proxy(None).await;
    let (target, cert) = tls_target().await;
    let client = Client::new(ClientConfig {
        extra_root_certificates: vec![cert],
        ..ClientConfig::default()
    })
    .unwrap();

    let req = via(
        format!("https://snap.test:{}/", target.port()),
        format!("http://{proxy}"),
    );
    let resp = client.execute(&req).await.unwrap();

    assert_eq!(
        resp.text().unwrap(),
        format!("host=snap.test:{}", target.port())
    );
    assert!(
        seen.lock().unwrap()[0]
            .starts_with(&format!("CONNECT snap.test:{} HTTP/1.1", target.port()))
    );
    assert!(
        infos(&resp.trace)
            .iter()
            .any(|m| m.starts_with("CONNECT tunnel to snap.test"))
    );
    assert!(resp.timings.tls.is_some());
}

#[tokio::test]
async fn rejected_connect_is_a_connect_error() {
    let (proxy, _) = http_proxy(Some("Basic nope")).await;
    let err = Client::default()
        .execute(&via("https://snap.test/".into(), format!("http://{proxy}")))
        .await
        .unwrap_err();
    let Error::Execution(err) = err else {
        panic!("{err:?}")
    };
    assert_eq!(err.phase, Phase::Connect);
    assert!(
        err.message.contains("407 Proxy Authentication Required"),
        "{}",
        err.message
    );
}

#[tokio::test]
async fn socks5h_lets_the_proxy_resolve_names() {
    let target = plain_target().await;
    let (proxy, seen) = socks5_proxy(None).await;
    let req = via(
        format!("http://far-away.test:{}/", target.port()),
        format!("socks5h://{proxy}"),
    );
    let resp = Client::default().execute(&req).await.unwrap();

    assert_eq!(
        resp.text().unwrap(),
        format!("host=far-away.test:{}", target.port())
    );
    assert_eq!(
        seen.lock().unwrap()[0],
        format!("far-away.test:{}", target.port())
    );
    assert!(!has_dns(&resp.trace));
}

#[tokio::test]
async fn socks5_resolves_locally_and_supports_auth() {
    let target = plain_target().await;
    let (proxy, seen) = socks5_proxy(Some(("alice", "pw"))).await;
    let req = via(
        format!("http://localhost:{}/", target.port()),
        format!("socks5://alice:pw@{proxy}"),
    );
    let resp = Client::default().execute(&req).await.unwrap();

    assert_eq!(resp.status, 200);
    let requested = seen.lock().unwrap()[0].clone();
    assert!(
        requested == format!("127.0.0.1:{}", target.port())
            || requested == format!("::1:{}", target.port()),
        "{requested}"
    );
    assert!(has_dns(&resp.trace));
}

#[tokio::test]
async fn socks5_wrong_password_fails() {
    let (proxy, _) = socks5_proxy(Some(("alice", "pw"))).await;
    let err = Client::default()
        .execute(&via(
            "http://x.test/".into(),
            format!("socks5h://alice:bad@{proxy}"),
        ))
        .await
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("rejected the username or password"),
        "{err}"
    );
}

#[tokio::test]
async fn direct_mode_ignores_proxies() {
    let target = plain_target().await;
    let mut req = Request::get(format!("http://{target}/"));
    req.settings.proxy = ProxyMode::Direct;
    let resp = Client::default().execute(&req).await.unwrap();
    assert!(infos(&resp.trace).is_empty());
}
