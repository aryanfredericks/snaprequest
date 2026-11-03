//! The outbound request policy is checked on every address the client connects to.
#![allow(clippy::unwrap_used)] // test helpers outside #[test] fns

use std::convert::Infallible;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use bytes::Bytes;
use http_body_util::Full;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use snapreq_core::model::{Phase, ResolveOverride, TraceEvent};
use snapreq_core::{Client, ClientConfig, ConnectPolicy, Error, Request};
use tokio::net::TcpListener;

#[derive(Debug, Default)]
struct Deny {
    hosts: Vec<&'static str>,
    ips: Vec<IpAddr>,
}

impl ConnectPolicy for Deny {
    fn check(&self, host: &str, ip: Option<IpAddr>) -> Result<(), String> {
        if self.hosts.contains(&host) {
            return Err(format!("host {host} is denied"));
        }
        if let Some(ip) = ip
            && self.ips.contains(&ip)
        {
            return Err("address is denied".into());
        }
        Ok(())
    }
}

/// `/to?to=URL` redirects; everything else answers `ok`.
async fn start() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let svc = service_fn(|req: hyper::Request<hyper::body::Incoming>| async move {
                    let to = req
                        .uri()
                        .query()
                        .and_then(|q| q.strip_prefix("to="))
                        .map(str::to_owned);
                    let resp = match to {
                        Some(to) => hyper::Response::builder()
                            .status(302)
                            .header("location", to)
                            .body(Full::new(Bytes::new())),
                        None => hyper::Response::builder().body(Full::new(Bytes::from("ok"))),
                    };
                    Ok::<_, Infallible>(resp.unwrap())
                });
                let _ = hyper::server::conn::http1::Builder::new()
                    .serve_connection(TokioIo::new(s), svc)
                    .await;
            });
        }
    });
    addr
}

fn client(policy: Deny) -> Client {
    Client::new(ClientConfig {
        connect_policy: Some(Arc::new(policy)),
        ..ClientConfig::default()
    })
    .unwrap()
}

fn connect_error(e: Error) -> snapreq_core::ExecutionError {
    match e {
        Error::Execution(e) => *e,
        other => panic!("expected an execution error, got {other}"),
    }
}

#[tokio::test]
async fn blocked_addresses_and_redirect_targets_fail() {
    let addr = start().await;
    let c = client(Deny {
        ips: vec!["127.0.0.1".parse().unwrap()],
        ..Deny::default()
    });
    let e = connect_error(
        c.execute(&Request::get(format!("http://{addr}/")))
            .await
            .unwrap_err(),
    );
    assert_eq!(e.phase, Phase::Connect);
    assert_eq!(
        e.message,
        "blocked by the outbound request policy: 127.0.0.1: address is denied"
    );

    // The first hop is allowed, the redirect target is not; pinned names are checked by name.
    let c = client(Deny {
        hosts: vec!["evil.test"],
        ..Deny::default()
    });
    let mut req = Request::get(format!(
        "http://{addr}/start?to=http://evil.test:{}/",
        addr.port()
    ));
    req.settings.resolve.push(
        format!("evil.test:{}:127.0.0.1", addr.port())
            .parse::<ResolveOverride>()
            .unwrap(),
    );
    let e = connect_error(c.execute(&req).await.unwrap_err());
    assert!(
        e.message.ends_with("host evil.test is denied"),
        "{}",
        e.message
    );
    assert!(
        e.trace
            .iter()
            .any(|t| matches!(t.event, TraceEvent::Redirect { .. }))
    );
}

#[tokio::test]
async fn blocked_addresses_of_a_name_are_skipped() {
    let addr = start().await;
    let c = client(Deny {
        ips: vec!["::1".parse().unwrap()],
        ..Deny::default()
    });
    let mut req = Request::get(format!("http://mixed.test:{}/", addr.port()));
    req.settings.resolve.push(
        format!("mixed.test:{}:[::1],127.0.0.1", addr.port())
            .parse::<ResolveOverride>()
            .unwrap(),
    );
    let resp = c.execute(&req).await.unwrap();
    assert_eq!(resp.text(), Some("ok"));
    assert!(resp.trace.iter().any(|t| matches!(&t.event, TraceEvent::Info { message } if message == "outbound request policy skipped ::1: address is denied")));
}
