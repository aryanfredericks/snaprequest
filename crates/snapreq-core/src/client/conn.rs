//! Opening connections (with per-step tracing) and keeping idle ones for reuse.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::net::TcpStream;
use url::{Host, Url};

use super::body::ReqBody;
use super::error_chain;
use super::meter::{Meter, MeteredIo};
use super::recorder::Recorder;
use crate::Result;
use crate::model::{Phase, TraceEvent};

/// Connections are shared only between requests to the same scheme, host and port.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PoolKey {
    scheme: String,
    host: String,
    port: u16,
}

impl PoolKey {
    pub(crate) fn new(url: &Url) -> Self {
        Self {
            scheme: url.scheme().to_owned(),
            host: url.host_str().unwrap_or_default().to_ascii_lowercase(),
            port: url.port_or_known_default().unwrap_or(80),
        }
    }
}

/// An open HTTP/1.1 connection.
#[derive(Debug)]
pub(crate) struct Conn {
    pub sender: http1::SendRequest<ReqBody>,
    pub meter: Meter,
    pub remote: SocketAddr,
}

#[derive(Debug)]
struct Idle {
    conn: Conn,
    since: Instant,
}

#[derive(Debug)]
pub(crate) struct Pool {
    idle: Mutex<HashMap<PoolKey, Vec<Idle>>>,
    idle_timeout: Duration,
    max_idle_per_host: usize,
}

impl Pool {
    pub(crate) fn new(idle_timeout: Duration, max_idle_per_host: usize) -> Self {
        Self {
            idle: Mutex::new(HashMap::new()),
            idle_timeout,
            max_idle_per_host,
        }
    }

    /// Takes the most recently used idle connection that still looks alive.
    pub(crate) fn checkout(&self, key: &PoolKey) -> Option<Conn> {
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        let list = idle.get_mut(key)?;
        while let Some(entry) = list.pop() {
            if entry.since.elapsed() < self.idle_timeout && !entry.conn.sender.is_closed() {
                return Some(entry.conn);
            }
        }
        None
    }

    /// Returns a connection for later reuse.
    pub(crate) fn checkin(&self, key: PoolKey, conn: Conn) {
        if conn.sender.is_closed() || self.max_idle_per_host == 0 {
            return;
        }
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        let list = idle.entry(key).or_default();
        list.retain(|e| e.since.elapsed() < self.idle_timeout && !e.conn.sender.is_closed());
        if list.len() < self.max_idle_per_host {
            list.push(Idle {
                conn,
                since: Instant::now(),
            });
        }
    }
}

/// Resolves, connects and performs the HTTP/1.1 handshake, recording each step.
pub(crate) async fn connect(url: &Url, rec: &mut Recorder) -> Result<Conn> {
    let port = url.port_or_known_default().unwrap_or(80);

    let addrs: Vec<SocketAddr> = match url.host() {
        Some(Host::Domain(domain)) => resolve(domain, port, rec).await?,
        Some(Host::Ipv4(ip)) => vec![SocketAddr::new(ip.into(), port)],
        Some(Host::Ipv6(ip)) => vec![SocketAddr::new(ip.into(), port)],
        None => return Err(rec.fail("URL has no host", false)),
    };

    rec.phase = Phase::Connect;
    let mut last_err = None;
    let mut connected = None;
    // Try addresses in resolver order; Happy Eyeballs (RFC 8305) comes with the custom resolver.
    for addr in &addrs {
        rec.record(TraceEvent::ConnectStart { addr: *addr });
        let started = Instant::now();
        match TcpStream::connect(addr).await {
            Ok(stream) => {
                let local_addr = stream
                    .local_addr()
                    .map_err(|e| rec.fail(format!("socket error: {e}"), false))?;
                rec.record(TraceEvent::ConnectEnd {
                    addr: *addr,
                    local_addr,
                    duration: started.elapsed(),
                });
                connected = Some((stream, *addr));
                break;
            }
            Err(e) => {
                rec.record(TraceEvent::Info {
                    message: format!("connect to {addr} failed: {e}"),
                });
                last_err = Some(e);
            }
        }
    }
    let Some((stream, remote)) = connected else {
        let why = last_err.map_or_else(|| "no addresses".to_owned(), |e| e.to_string());
        return Err(rec.fail(
            format!("could not connect to {}:{port}: {why}", host_display(url)),
            false,
        ));
    };
    // Disabling Nagle avoids an artificial delay between headers and a small body.
    let _ = stream.set_nodelay(true);

    let meter = Meter::default();
    let io = TokioIo::new(MeteredIo::new(stream, meter.clone()));
    let (sender, connection) = http1::Builder::new()
        .title_case_headers(true)
        .handshake(io)
        .await
        .map_err(|e| rec.fail(format!("HTTP handshake failed: {}", error_chain(&e)), false))?;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::debug!(error = %e, %remote, "connection closed with error");
        }
    });

    Ok(Conn {
        sender,
        meter,
        remote,
    })
}

async fn resolve(domain: &str, port: u16, rec: &mut Recorder) -> Result<Vec<SocketAddr>> {
    rec.phase = Phase::Dns;
    rec.record(TraceEvent::DnsStart {
        host: domain.to_owned(),
    });
    let started = Instant::now();
    let found = tokio::net::lookup_host((domain, port))
        .await
        .map_err(|e| rec.fail(format!("could not resolve `{domain}`: {e}"), false))?;

    let mut addrs: Vec<SocketAddr> = Vec::new();
    for a in found {
        if !addrs.contains(&a) {
            addrs.push(a);
        }
    }
    rec.record(TraceEvent::DnsEnd {
        host: domain.to_owned(),
        addrs: addrs.iter().map(SocketAddr::ip).collect(),
        duration: started.elapsed(),
    });
    if addrs.is_empty() {
        return Err(rec.fail(format!("`{domain}` has no addresses"), false));
    }
    Ok(addrs)
}

fn host_display(url: &Url) -> &str {
    url.host_str().unwrap_or_default()
}
