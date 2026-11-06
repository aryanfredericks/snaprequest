//! Opening connections (with per-step tracing) and keeping idle ones for reuse.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Mutex;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use futures_util::stream::FuturesUnordered;

use hyper::client::conn::{http1, http2};
use hyper_util::rt::{TokioExecutor, TokioIo};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::TcpStream;
use tokio_rustls::client::TlsStream;
use url::{Host, Url};

use super::body::ReqBody;
use super::error_chain;
use super::meter::{Meter, MeteredIo};
use super::proxy::{self, Proxy, ProxyKind};
use super::recorder::Recorder;
use super::tls::{self, RequestTls};
use crate::Result;
use crate::model::{HttpVersionPref, Phase, RequestSettings, TlsSettings, TraceEvent};

/// Connections are shared only between requests to the same scheme, host, port and
/// HTTP version preference, and (for HTTPS) the same TLS settings, so a connection
/// opened with verification off is never handed to a request that verifies.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct PoolKey {
    scheme: String,
    host: String,
    port: u16,
    version: HttpVersionPref,
    tls: Option<TlsSettings>,
    proxy: Option<Proxy>,
}

impl PoolKey {
    pub(crate) fn new(url: &Url, settings: &RequestSettings, proxy: Option<&Proxy>) -> Self {
        Self {
            scheme: url.scheme().to_owned(),
            host: url.host_str().unwrap_or_default().to_ascii_lowercase(),
            port: url.port_or_known_default().unwrap_or(80),
            version: settings.http_version,
            tls: (url.scheme() == "https").then(|| settings.tls.clone()),
            proxy: proxy.cloned(),
        }
    }
}

/// The request side of an HTTP/1.1 or HTTP/2 connection.
#[derive(Debug)]
pub(crate) enum Sender {
    H1(http1::SendRequest<ReqBody>),
    /// Cloneable: every clone opens new streams on the same connection.
    H2(http2::SendRequest<ReqBody>),
}

impl Sender {
    pub(crate) fn is_h2(&self) -> bool {
        matches!(self, Self::H2(_))
    }

    pub(crate) fn is_closed(&self) -> bool {
        match self {
            Self::H1(s) => s.is_closed(),
            Self::H2(s) => s.is_closed(),
        }
    }

    pub(crate) async fn ready(&mut self) -> std::result::Result<(), hyper::Error> {
        match self {
            Self::H1(s) => s.ready().await,
            Self::H2(s) => s.ready().await,
        }
    }

    pub(crate) async fn send(
        &mut self,
        req: http::Request<ReqBody>,
    ) -> std::result::Result<http::Response<hyper::body::Incoming>, hyper::Error> {
        match self {
            Self::H1(s) => s.send_request(req).await,
            Self::H2(s) => s.send_request(req).await,
        }
    }
}

/// An open connection.
#[derive(Debug)]
pub(crate) struct Conn {
    pub sender: Sender,
    pub meter: Meter,
    pub remote: SocketAddr,
    /// Set when requests go to an HTTP proxy in absolute form (plain-HTTP targets):
    /// the `Proxy-Authorization` value to add, if any.
    pub forward_proxy: Option<Option<String>>,
}

impl Conn {
    /// Another handle to the same HTTP/2 connection.
    fn share(&self) -> Option<Self> {
        match &self.sender {
            Sender::H2(s) => Some(Self {
                sender: Sender::H2(s.clone()),
                meter: self.meter.clone(),
                remote: self.remote,
                forward_proxy: self.forward_proxy.clone(),
            }),
            Sender::H1(_) => None,
        }
    }
}

#[derive(Debug)]
struct Idle {
    conn: Conn,
    since: Instant,
}

/// Idle HTTP/1.1 connections are checked out exclusively; HTTP/2 connections are
/// shared by every request with the same key.
#[derive(Debug)]
pub(crate) struct Pool {
    idle: Mutex<HashMap<PoolKey, Vec<Idle>>>,
    shared: Mutex<HashMap<PoolKey, Idle>>,
    idle_timeout: Duration,
    max_idle_per_host: usize,
}

impl Pool {
    pub(crate) fn new(idle_timeout: Duration, max_idle_per_host: usize) -> Self {
        Self {
            idle: Mutex::new(HashMap::new()),
            shared: Mutex::new(HashMap::new()),
            idle_timeout,
            max_idle_per_host,
        }
    }

    /// A shared HTTP/2 connection, or the most recently used idle HTTP/1.1 connection
    /// that still looks alive.
    pub(crate) fn checkout(&self, key: &PoolKey) -> Option<Conn> {
        if self.max_idle_per_host == 0 {
            return None;
        }
        {
            let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = shared.get_mut(key) {
                if entry.since.elapsed() < self.idle_timeout && !entry.conn.sender.is_closed() {
                    entry.since = Instant::now();
                    return entry.conn.share();
                }
                shared.remove(key);
            }
        }
        let mut idle = self.idle.lock().unwrap_or_else(|e| e.into_inner());
        let list = idle.get_mut(key)?;
        while let Some(entry) = list.pop() {
            if entry.since.elapsed() < self.idle_timeout && !entry.conn.sender.is_closed() {
                return Some(entry.conn);
            }
        }
        None
    }

    /// Makes a new HTTP/2 connection available to concurrent requests right away.
    pub(crate) fn share(&self, key: &PoolKey, conn: &Conn) {
        if self.max_idle_per_host == 0 {
            return;
        }
        if let Some(handle) = conn.share() {
            let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            shared.insert(
                key.clone(),
                Idle {
                    conn: handle,
                    since: Instant::now(),
                },
            );
        }
    }

    /// Returns a connection for later reuse. HTTP/2 handles are already shared, so
    /// only their idle timer is refreshed.
    pub(crate) fn checkin(&self, key: PoolKey, conn: Conn) {
        if conn.sender.is_closed() || self.max_idle_per_host == 0 {
            return;
        }
        if conn.sender.is_h2() {
            let mut shared = self.shared.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(entry) = shared.get_mut(&key) {
                entry.since = Instant::now();
            }
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

/// Resolves, connects, does TLS for `https`, and performs the HTTP/1.1 or HTTP/2
/// handshake, recording each step.
///
/// HTTP/2 is used when TLS negotiates `h2` through ALPN, or over plain TCP when the
/// request asks for HTTP/2 (prior knowledge).
pub(crate) async fn connect(
    url: &Url,
    settings: &RequestSettings,
    tls: Option<&RequestTls>,
    proxy: Option<&Proxy>,
    policy: Option<&dyn super::ConnectPolicy>,
    rec: &mut Recorder,
) -> Result<Conn> {
    let port = url.port_or_known_default().unwrap_or(80);
    let target = url
        .host()
        .ok_or_else(|| rec.fail("URL has no host", false))?;

    let mut forward_proxy = None;
    let (mut tcp, remote) = match proxy {
        None => {
            let host = url.host_str().unwrap_or_default();
            let addrs = addresses(target.clone(), port, settings, rec).await?;
            let addrs = allowed(policy, host, addrs, rec)?;
            happy_eyeballs(&addrs, host, port, rec).await?
        }
        Some(proxy) => {
            rec.record(TraceEvent::Info {
                message: format!("using proxy {} (from {})", proxy.display(), proxy.source),
            });
            let addrs = addresses(proxy.host(), proxy.port, settings, rec).await?;
            let addrs = allowed(policy, &proxy.host, addrs, rec)?;
            // The target is reached through the proxy; check what is known about it.
            if let Some(policy) = policy
                && !matches!(proxy.kind, ProxyKind::Socks5 if matches!(target, Host::Domain(_)))
            {
                let host = url.host_str().unwrap_or_default();
                let ip = match &target {
                    Host::Ipv4(v4) => Some((*v4).into()),
                    Host::Ipv6(v6) => Some((*v6).into()),
                    Host::Domain(_) => None,
                };
                rec.phase = Phase::Connect;
                policy
                    .check(host.trim_matches(['[', ']']), ip)
                    .map_err(|why| {
                        rec.fail(
                            format!("blocked by the outbound request policy: {host}: {why}"),
                            false,
                        )
                    })?;
            }
            happy_eyeballs(&addrs, &proxy.host, proxy.port, rec).await?
        }
    };
    // Disabling Nagle avoids an artificial delay between headers and a small body.
    let _ = tcp.set_nodelay(true);

    if let Some(proxy) = proxy {
        let authority = format!("{}:{port}", url.host_str().unwrap_or_default());
        let started = Instant::now();
        match proxy.kind {
            ProxyKind::Http if url.scheme() == "http" => {
                forward_proxy = Some(proxy.basic_authorization());
            }
            ProxyKind::Http => {
                proxy::http_connect(&mut tcp, &authority, proxy)
                    .await
                    .map_err(|e| rec.fail(e, false))?;
                rec.record(TraceEvent::Info {
                    message: format!(
                        "CONNECT tunnel to {authority} established in {} ms",
                        started.elapsed().as_millis()
                    ),
                });
            }
            ProxyKind::Socks5 | ProxyKind::Socks5h => {
                let ip;
                let socks_target = match (proxy.kind, target.clone()) {
                    (ProxyKind::Socks5h, Host::Domain(d)) => proxy::SocksTarget::Domain(d),
                    (_, Host::Domain(_)) => {
                        // socks5://: resolve locally, like curl.
                        let host = url.host_str().unwrap_or_default();
                        let found = addresses(target.clone(), port, settings, rec).await?;
                        ip = allowed(policy, host, found, rec)?[0].ip();
                        rec.phase = Phase::Connect;
                        proxy::SocksTarget::Ip(ip)
                    }
                    (_, Host::Ipv4(v4)) => proxy::SocksTarget::Ip(v4.into()),
                    (_, Host::Ipv6(v6)) => proxy::SocksTarget::Ip(v6.into()),
                };
                proxy::socks5_connect(&mut tcp, socks_target, port, proxy.credentials.as_ref())
                    .await
                    .map_err(|e| rec.fail(e, false))?;
                rec.record(TraceEvent::Info {
                    message: format!(
                        "SOCKS5 tunnel to {authority} established in {} ms",
                        started.elapsed().as_millis()
                    ),
                });
            }
        }
    }

    let (stream, use_h2) = match tls {
        Some(tls) => {
            let stream = tls::handshake(tls, tcp, url, rec).await?;
            let h2 = stream.get_ref().1.alpn_protocol() == Some(b"h2");
            if settings.http_version == HttpVersionPref::Http2 && !h2 {
                return Err(rec.fail(
                    "the server does not support HTTP/2 (ALPN did not select `h2`)",
                    false,
                ));
            }
            (MaybeTls::Tls(Box::new(stream)), h2)
        }
        // Prior-knowledge HTTP/2 is not forwarded through HTTP proxies.
        None => (
            MaybeTls::Plain(tcp),
            settings.http_version == HttpVersionPref::Http2 && forward_proxy.is_none(),
        ),
    };

    let meter = Meter::default();
    let io = TokioIo::new(MeteredIo::new(stream, meter.clone()));
    let handshake_failed = |e: hyper::Error| format!("HTTP handshake failed: {}", error_chain(&e));
    let sender = if use_h2 {
        let (sender, connection) = http2::Builder::new(TokioExecutor::new())
            .handshake(io)
            .await
            .map_err(|e| rec.fail(handshake_failed(e), false))?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                tracing::debug!(error = %e, %remote, "HTTP/2 connection closed with error");
            }
        });
        Sender::H2(sender)
    } else {
        let (sender, connection) = http1::Builder::new()
            .title_case_headers(true)
            .handshake(io)
            .await
            .map_err(|e| rec.fail(handshake_failed(e), false))?;
        tokio::spawn(async move {
            // With upgrades, a `101 Switching Protocols` hands the socket to the WebSocket client.
            if let Err(e) = connection.with_upgrades().await {
                tracing::debug!(error = %e, %remote, "connection closed with error");
            }
        });
        Sender::H1(sender)
    };

    Ok(Conn {
        sender,
        meter,
        remote,
        forward_proxy,
    })
}

/// The addresses `policy` allows. Blocked ones are noted in the trace; if none is left,
/// the request fails with the reason.
fn allowed(
    policy: Option<&dyn super::ConnectPolicy>,
    host: &str,
    addrs: Vec<SocketAddr>,
    rec: &mut Recorder,
) -> Result<Vec<SocketAddr>> {
    let Some(policy) = policy else {
        return Ok(addrs);
    };
    let host = host.trim_matches(['[', ']']);
    let mut kept = Vec::new();
    let mut reasons = Vec::new();
    for a in addrs {
        match policy.check(host, Some(a.ip())) {
            Ok(()) => kept.push(a),
            Err(why) => reasons.push(format!("{}: {why}", a.ip())),
        }
    }
    if kept.is_empty() {
        rec.phase = Phase::Connect;
        return Err(rec.fail(
            format!(
                "blocked by the outbound request policy: {}",
                reasons.join("; ")
            ),
            false,
        ));
    }
    if !reasons.is_empty() {
        rec.record(TraceEvent::Info {
            message: format!("outbound request policy skipped {}", reasons.join("; ")),
        });
    }
    Ok(kept)
}

/// Addresses for `host`: pinned addresses, a DNS lookup, or the literal IP.
async fn addresses(
    host: Host<&str>,
    port: u16,
    settings: &RequestSettings,
    rec: &mut Recorder,
) -> Result<Vec<SocketAddr>> {
    let addrs: Vec<SocketAddr> = match host {
        Host::Domain(domain) => match settings.resolve.iter().find(|o| o.matches(domain, port)) {
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
        },
        Host::Ipv4(ip) => vec![SocketAddr::new(ip.into(), port)],
        Host::Ipv6(ip) => vec![SocketAddr::new(ip.into(), port)],
    };
    if addrs.is_empty() {
        return Err(rec.fail("no addresses to connect to", false));
    }
    Ok(addrs)
}

/// Connects to the first address that answers. Address families are interleaved and
/// a new attempt starts every [`ATTEMPT_DELAY`] (or immediately after a failure) while
/// earlier ones are still pending, so one dead address (often a broken IPv6 route)
/// costs 250 ms instead of a full connect timeout.
async fn happy_eyeballs(
    addrs: &[SocketAddr],
    host: &str,
    port: u16,
    rec: &mut Recorder,
) -> Result<(TcpStream, SocketAddr)> {
    rec.phase = Phase::Connect;
    let order = interleave_families(addrs);
    let mut attempts = FuturesUnordered::new();
    let mut in_flight: Vec<SocketAddr> = Vec::new();
    let mut next = 0;
    let mut last_err: Option<std::io::Error> = None;
    // The first attempt starts immediately: even a zero-length Tokio sleep waits for the
    // next 1 ms timer tick, which would add a millisecond to every new connection.
    let delay = tokio::time::sleep(ATTEMPT_DELAY);
    tokio::pin!(delay);

    // Each pass starts one attempt: the loop only comes around again when an attempt
    // failed or the stagger timer fired, and both mean "try the next address now".
    loop {
        if next < order.len() {
            let addr = order[next];
            next += 1;
            rec.record(TraceEvent::ConnectStart { addr });
            in_flight.push(addr);
            attempts.push(async move {
                let started = Instant::now();
                (addr, started, TcpStream::connect(addr).await)
            });
            delay
                .as_mut()
                .reset(tokio::time::Instant::now() + ATTEMPT_DELAY);
        }
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
                    }
                }
            }
            // Stagger: the pending attempt is slow, start the next one in parallel.
            () = &mut delay, if next < order.len() => {}
            else => break,
        }
    }

    let why = last_err.map_or_else(|| "no addresses".to_owned(), |e| e.to_string());
    Err(rec.fail(format!("could not connect to {host}:{port}: {why}"), false))
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
