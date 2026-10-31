//! Opening connections (with per-step tracing) and keeping idle ones for reuse.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;

use hyper::client::conn::http1;
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use url::{Host, Url};

use super::body::ReqBody;
use super::error_chain;
use super::meter::{Meter, MeteredIo};
use super::recorder::Recorder;
use super::tls::{self, RequestTls};
use crate::Result;
use crate::model::{Phase, RequestSettings, TlsSettings, TraceEvent};

/// Connections are shared only between requests to the same scheme, host and port,
/// and (for HTTPS) the same TLS settings, so a connection opened with verification
/// off is never handed to a request that verifies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PoolKey {
    scheme: String,
    host: String,
    port: u16,
    tls: Option<TlsSettings>,
}

impl PoolKey {
    pub(crate) fn new(url: &Url, tls: &TlsSettings) -> Self {
        Self {
            scheme: url.scheme().to_owned(),
            host: url.host_str().unwrap_or_default().to_ascii_lowercase(),
            port: url.port_or_known_default().unwrap_or(80),
            tls: (url.scheme() == "https").then(|| tls.clone()),
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

/// Delay before starting the next connection attempt while one is still pending
/// (RFC 8305 recommends 250 ms).
const ATTEMPT_DELAY: Duration = Duration::from_millis(250);

/// Resolves, connects, does TLS for `https`, and performs the HTTP/1.1 handshake,
/// recording each step.
pub(crate) async fn connect(
    url: &Url,
    settings: &RequestSettings,
    tls: Option<&RequestTls>,
    rec: &mut Recorder,
) -> Result<Conn> {
    let port = url.port_or_known_default().unwrap_or(80);

    let addrs: Vec<SocketAddr> = match url.host() {
        Some(Host::Domain(domain)) => {
            match settings.resolve.iter().find(|o| o.matches(domain, port)) {
                Some(pinned) => {
                    rec.record(TraceEvent::Info {
                        message: format!(
                            "{domain}:{port} pinned to {} (DNS skipped)",
                            join(&pinned.addrs)
                        ),
                    });
                    pinned
                        .addrs
                        .iter()
                        .map(|ip| SocketAddr::new(*ip, port))
                        .collect()
                }
                None => resolve(domain, port, rec).await?,
            }
        }
        Some(Host::Ipv4(ip)) => vec![SocketAddr::new(ip.into(), port)],
        Some(Host::Ipv6(ip)) => vec![SocketAddr::new(ip.into(), port)],
        None => return Err(rec.fail("URL has no host", false)),
    };
    if addrs.is_empty() {
        return Err(rec.fail("no addresses to connect to", false));
    }

    let (tcp, remote) = happy_eyeballs(&addrs, url, port, rec).await?;
    // Disabling Nagle avoids an artificial delay between headers and a small body.
    let _ = tcp.set_nodelay(true);

    let stream = match tls {
        Some(tls) => MaybeTls::Tls(Box::new(tls::handshake(tls, tcp, url, rec).await?)),
        None => MaybeTls::Plain(tcp),
    };

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

/// Connects to the first address that answers. Address families are interleaved and
/// a new attempt starts every [`ATTEMPT_DELAY`] (or immediately after a failure) while
/// earlier ones are still pending, so one dead address (often a broken IPv6 route)
/// costs 250 ms instead of a full connect timeout.
async fn happy_eyeballs(
    addrs: &[SocketAddr],
    url: &Url,
    port: u16,
    rec: &mut Recorder,
) -> Result<(TcpStream, SocketAddr)> {
    rec.phase = Phase::Connect;
    let order = interleave_families(addrs);
    let mut attempts = FuturesUnordered::new();
    let mut in_flight: Vec<SocketAddr> = Vec::new();
    let mut next = 0;
    let mut last_err: Option<std::io::Error> = None;
    let delay = tokio::time::sleep(Duration::ZERO);
    tokio::pin!(delay);

    loop {
        tokio::select! {
            biased;
            Some((addr, started, result)) = attempts.next(), if !attempts.is_empty() => {
                in_flight.retain(|a| *a != addr);
                let elapsed: Duration = Instant::now().saturating_duration_since(started);
                match result {
                    Ok(stream) => {
                        let stream: TcpStream = stream;
                        let local_addr = stream
                            .local_addr()
                            .map_err(|e| rec.fail(format!("socket error: {e}"), false))?;
                        rec.record(TraceEvent::ConnectEnd { addr, local_addr, duration: elapsed });
                        for abandoned in in_flight {
                            rec.record(TraceEvent::Info {
                                message: format!("abandoned slower connection attempt to {abandoned}"),
                            });
                        }
                        return Ok((stream, addr));
                    }
                    Err(e) => {
                        rec.record(TraceEvent::Info {
                            message: format!("connect to {addr} failed after {} ms: {e}", elapsed.as_millis()),
                        });
                        last_err = Some(e);
                        // Try the next address right away.
                        delay.as_mut().reset(tokio::time::Instant::now());
                    }
                }
            }
            () = &mut delay, if next < order.len() => {
                let addr = order[next];
                next += 1;
                rec.record(TraceEvent::ConnectStart { addr });
                in_flight.push(addr);
                attempts.push(async move {
                    let started = Instant::now();
                    (addr, started, TcpStream::connect(addr).await)
                });
                delay.as_mut().reset(tokio::time::Instant::now() + ATTEMPT_DELAY);
            }
            else => break,
        }
    }

    let why = last_err.map_or_else(|| "no addresses".to_owned(), |e| e.to_string());
    Err(rec.fail(
        format!(
            "could not connect to {}:{port}: {why}",
            url.host_str().unwrap_or_default()
        ),
        false,
    ))
}

/// Reorders addresses to alternate families, starting with the family of the
/// first address (the resolver's preference), as RFC 8305 section 4 suggests.
fn interleave_families(addrs: &[SocketAddr]) -> Vec<SocketAddr> {
    let Some(first) = addrs.first() else {
        return Vec::new();
    };
    let (mut preferred, mut other): (Vec<_>, Vec<_>) =
        addrs.iter().partition(|a| a.is_ipv6() == first.is_ipv6());
    preferred.reverse();
    other.reverse();
    let mut out = Vec::with_capacity(addrs.len());
    loop {
        match (preferred.pop(), other.pop()) {
            (None, None) => return out,
            (a, b) => out.extend(a.into_iter().chain(b).copied()),
        }
    }
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

fn join(addrs: &[std::net::IpAddr]) -> String {
    addrs
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
}

/// A plain TCP or TLS stream.
#[derive(Debug)]
pub(crate) enum MaybeTls {
    Plain(TcpStream),
    Tls(Box<TlsStream<TcpStream>>),
}

impl AsyncRead for MaybeTls {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_read(cx, buf),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_read(cx, buf),
        }
    }
}

impl AsyncWrite for MaybeTls {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write(cx, buf),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_write(cx, buf),
        }
    }

    fn poll_write_vectored(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        bufs: &[std::io::IoSlice<'_>],
    ) -> Poll<std::io::Result<usize>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_write_vectored(cx, bufs),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_write_vectored(cx, bufs),
        }
    }

    fn is_write_vectored(&self) -> bool {
        match self {
            Self::Plain(s) => s.is_write_vectored(),
            Self::Tls(s) => s.is_write_vectored(),
        }
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_flush(cx),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_flush(cx),
        }
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        match self.get_mut() {
            Self::Plain(s) => Pin::new(s).poll_shutdown(cx),
            Self::Tls(s) => Pin::new(s.as_mut()).poll_shutdown(cx),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn addrs(list: &[&str]) -> Vec<SocketAddr> {
        list.iter().map(|a| a.parse().unwrap()).collect()
    }

    #[test]
    fn families_are_interleaved_starting_with_the_preferred_one() {
        let input = addrs(&[
            "[::1]:80",
            "[::2]:80",
            "[::3]:80",
            "10.0.0.1:80",
            "10.0.0.2:80",
        ]);
        assert_eq!(
            interleave_families(&input),
            addrs(&[
                "[::1]:80",
                "10.0.0.1:80",
                "[::2]:80",
                "10.0.0.2:80",
                "[::3]:80"
            ])
        );
        let v4_first = addrs(&["10.0.0.1:80", "[::1]:80"]);
        assert_eq!(interleave_families(&v4_first), v4_first);
        assert!(interleave_families(&[]).is_empty());
    }
}
