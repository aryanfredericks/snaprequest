//! Proxies: choosing one (explicit or from the environment) and opening tunnels
//! through HTTP `CONNECT` and SOCKS5.

use std::net::IpAddr;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use url::{Host, Url};

use crate::model::ProxyMode;

/// Kinds of proxy SnapRequest can use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ProxyKind {
    /// HTTP proxy: absolute-form requests for `http`, `CONNECT` tunnels for `https`.
    Http,
    /// SOCKS5; SnapRequest resolves the target name.
    Socks5,
    /// SOCKS5; the proxy resolves the target name.
    Socks5h,
}

/// A proxy to connect through.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct Proxy {
    pub kind: ProxyKind,
    pub host: String,
    pub port: u16,
    pub credentials: Option<(String, String)>,
    /// Where the proxy came from (`settings` or the environment variable name).
    pub source: String,
}

impl Proxy {
    /// URL for display, with the password hidden.
    pub(crate) fn display(&self) -> String {
        let scheme = match self.kind {
            ProxyKind::Http => "http",
            ProxyKind::Socks5 => "socks5",
            ProxyKind::Socks5h => "socks5h",
        };
        let user = match &self.credentials {
            Some((user, _)) => format!("{user}:***@"),
            None => String::new(),
        };
        format!("{scheme}://{user}{}:{}", self.host, self.port)
    }

    /// `Proxy-Authorization` value for HTTP proxies with credentials.
    pub(crate) fn basic_authorization(&self) -> Option<String> {
        self.credentials
            .as_ref()
            .map(|(u, p)| format!("Basic {}", STANDARD.encode(format!("{u}:{p}"))))
    }

    /// The proxy's own host, as a `url::Host`.
    pub(crate) fn host(&self) -> Host<&str> {
        match self.host.parse::<IpAddr>() {
            Ok(IpAddr::V4(ip)) => Host::Ipv4(ip),
            Ok(IpAddr::V6(ip)) => Host::Ipv6(ip),
            Err(_) => Host::Domain(&self.host),
        }
    }
}

/// Picks the proxy for `url`. `env` looks up environment variables; it is a parameter
/// so tests never have to modify the real process environment.
pub(crate) fn select(
    url: &Url,
    mode: &ProxyMode,
    env: impl Fn(&str) -> Option<String>,
) -> Result<Option<Proxy>, String> {
    match mode {
        ProxyMode::Direct => Ok(None),
        ProxyMode::Url(raw) => parse(raw, "settings").map(Some),
        ProxyMode::Environment => {
            if is_loopback(url) {
                return Ok(None);
            }
            let keys: &[&str] = if url.scheme() == "https" {
                &["https_proxy", "HTTPS_PROXY", "all_proxy", "ALL_PROXY"]
            } else {
                &["http_proxy", "HTTP_PROXY", "all_proxy", "ALL_PROXY"]
            };
            let Some((key, value)) = keys
                .iter()
                .find_map(|k| env(k).filter(|v| !v.trim().is_empty()).map(|v| (*k, v)))
            else {
                return Ok(None);
            };
            let no_proxy = env("no_proxy")
                .or_else(|| env("NO_PROXY"))
                .unwrap_or_default();
            if bypasses(url.host_str().unwrap_or_default(), &no_proxy) {
                return Ok(None);
            }
            // `proxy:3128` without a scheme is common in environment variables.
            let value = value.trim();
            let value = if value.contains("://") {
                value.to_owned()
            } else {
                format!("http://{value}")
            };
            parse(&value, key)
                .map(Some)
                .map_err(|e| format!("{e} (from {key})"))
        }
    }
}

fn parse(raw: &str, source: &str) -> Result<Proxy, String> {
    let url = Url::parse(raw.trim()).map_err(|e| format!("invalid proxy URL `{raw}`: {e}"))?;
    let kind = match url.scheme() {
        "http" => ProxyKind::Http,
        "socks5" => ProxyKind::Socks5,
        "socks5h" => ProxyKind::Socks5h,
        other => {
            return Err(format!(
                "unsupported proxy scheme `{other}` (use http, socks5 or socks5h)"
            ));
        }
    };
    let host = url
        .host_str()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| format!("proxy URL `{raw}` has no host"))?
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_owned();
    let port = url
        .port()
        .unwrap_or(if kind == ProxyKind::Http { 80 } else { 1080 });
    let credentials = (!url.username().is_empty()).then(|| {
        (
            decode(url.username()),
            decode(url.password().unwrap_or_default()),
        )
    });
    Ok(Proxy {
        kind,
        host,
        port,
        credentials,
        source: source.to_owned(),
    })
}

fn decode(s: &str) -> String {
    percent_encoding::percent_decode_str(s)
        .decode_utf8_lossy()
        .into_owned()
}

fn is_loopback(url: &Url) -> bool {
    match url.host() {
        Some(Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    }
}

/// `NO_PROXY` matching: `*` matches everything; `example.com` and `.example.com`
/// match the domain and its subdomains; IP addresses match exactly.
fn bypasses(host: &str, no_proxy: &str) -> bool {
    let host = host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .to_ascii_lowercase();
    no_proxy
        .split(',')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| !e.is_empty())
        .any(|entry| {
            if entry == "*" {
                return true;
            }
            let entry = entry.trim_start_matches('.');
            // Ignore an optional port (`host:8080`) unless the entry is an IPv6 address.
            let entry = match entry.rsplit_once(':') {
                Some((h, p)) if entry.parse::<IpAddr>().is_err() && p.parse::<u16>().is_ok() => h,
                _ => entry,
            };
            host == entry || host.ends_with(&format!(".{entry}"))
        })
}

/// Opens a tunnel to `authority` (`host:port`) with HTTP `CONNECT`.
pub(crate) async fn http_connect(
    stream: &mut TcpStream,
    authority: &str,
    proxy: &Proxy,
) -> Result<(), String> {
    let mut request = format!("CONNECT {authority} HTTP/1.1\r\nHost: {authority}\r\n");
    if let Some(auth) = proxy.basic_authorization() {
        request.push_str(&format!("Proxy-Authorization: {auth}\r\n"));
    }
    request.push_str("\r\n");
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("could not send CONNECT to the proxy: {e}"))?;

    let mut head = Vec::new();
    let mut chunk = [0u8; 1024];
    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
        let n = stream
            .read(&mut chunk)
            .await
            .map_err(|e| format!("proxy closed the CONNECT exchange: {e}"))?;
        if n == 0 {
            return Err("proxy closed the connection during CONNECT".into());
        }
        head.extend_from_slice(&chunk[..n]);
        if head.len() > 64 * 1024 {
            return Err("proxy CONNECT response headers are too large".into());
        }
    }
    let head = String::from_utf8_lossy(&head);
    let status_line = head.lines().next().unwrap_or_default().trim().to_owned();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse::<u16>().ok());
    match status {
        Some(200..=299) => Ok(()),
        _ => Err(format!("proxy refused the tunnel: `{status_line}`")),
    }
}

/// Target of a SOCKS5 `CONNECT`.
pub(crate) enum SocksTarget<'a> {
    Domain(&'a str),
    Ip(IpAddr),
}

/// Performs the SOCKS5 handshake (RFC 1928, with RFC 1929 username/password auth).
pub(crate) async fn socks5_connect(
    stream: &mut TcpStream,
    target: SocksTarget<'_>,
    port: u16,
    credentials: Option<&(String, String)>,
) -> Result<(), String> {
    let io = |e: std::io::Error| format!("SOCKS5 handshake failed: {e}");

    let greeting: &[u8] = if credentials.is_some() {
        &[5, 2, 0, 2]
    } else {
        &[5, 1, 0]
    };
    stream.write_all(greeting).await.map_err(io)?;
    let mut choice = [0u8; 2];
    stream.read_exact(&mut choice).await.map_err(io)?;
    if choice[0] != 5 {
        return Err("the proxy did not answer as a SOCKS5 server".into());
    }
    match (choice[1], credentials) {
        (0, _) => {}
        (2, Some((user, pass))) => {
            if user.len() > 255 || pass.len() > 255 {
                return Err("SOCKS5 username and password must be at most 255 bytes".into());
            }
            let mut auth = vec![1, user.len() as u8];
            auth.extend_from_slice(user.as_bytes());
            auth.push(pass.len() as u8);
            auth.extend_from_slice(pass.as_bytes());
            stream.write_all(&auth).await.map_err(io)?;
            let mut status = [0u8; 2];
            stream.read_exact(&mut status).await.map_err(io)?;
            if status[1] != 0 {
                return Err("SOCKS5 proxy rejected the username or password".into());
            }
        }
        (0xFF, _) | (2, None) => {
            return Err("SOCKS5 proxy accepted none of the offered authentication methods".into());
        }
        (other, _) => return Err(format!("SOCKS5 proxy chose unknown auth method {other}")),
    }

    let mut request = vec![5, 1, 0];
    match target {
        SocksTarget::Domain(d) => {
            if d.len() > 255 {
                return Err("host name too long for SOCKS5".into());
            }
            request.push(3);
            request.push(d.len() as u8);
            request.extend_from_slice(d.as_bytes());
        }
        SocksTarget::Ip(IpAddr::V4(ip)) => {
            request.push(1);
            request.extend_from_slice(&ip.octets());
        }
        SocksTarget::Ip(IpAddr::V6(ip)) => {
            request.push(4);
            request.extend_from_slice(&ip.octets());
        }
    }
    request.extend_from_slice(&port.to_be_bytes());
    stream.write_all(&request).await.map_err(io)?;

    let mut reply = [0u8; 4];
    stream.read_exact(&mut reply).await.map_err(io)?;
    if reply[1] != 0 {
        return Err(format!(
            "SOCKS5 proxy could not connect: {}",
            socks_reply(reply[1])
        ));
    }
    // Skip the bound address and port.
    let skip = match reply[3] {
        1 => 4 + 2,
        4 => 16 + 2,
        3 => {
            let mut len = [0u8; 1];
            stream.read_exact(&mut len).await.map_err(io)?;
            usize::from(len[0]) + 2
        }
        other => return Err(format!("SOCKS5 reply has unknown address type {other}")),
    };
    let mut rest = vec![0u8; skip];
    stream.read_exact(&mut rest).await.map_err(io)?;
    Ok(())
}

fn socks_reply(code: u8) -> &'static str {
    match code {
        1 => "general failure",
        2 => "connection not allowed by ruleset",
        3 => "network unreachable",
        4 => "host unreachable",
        5 => "connection refused",
        6 => "TTL expired",
        7 => "command not supported",
        8 => "address type not supported",
        _ => "unknown error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        move |k| map.get(k).cloned()
    }

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn explicit_proxy_with_credentials() {
        let p = select(
            &url("https://api.test/"),
            &ProxyMode::Url("http://me:p%40ss@proxy.test:3128".into()),
            env(&[]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(p.kind, ProxyKind::Http);
        assert_eq!((p.host.as_str(), p.port), ("proxy.test", 3128));
        assert_eq!(p.credentials, Some(("me".into(), "p@ss".into())));
        assert_eq!(p.display(), "http://me:***@proxy.test:3128");
        assert_eq!(p.basic_authorization().unwrap(), "Basic bWU6cEBzcw==");
    }

    #[test]
    fn environment_proxy_by_scheme() {
        let vars = env(&[
            ("HTTP_PROXY", "plain.proxy:8080"),
            ("https_proxy", "socks5h://secure.proxy"),
        ]);
        let http = select(&url("http://a.test/"), &ProxyMode::Environment, &vars)
            .unwrap()
            .unwrap();
        assert_eq!(
            (http.kind, http.port, http.source.as_str()),
            (ProxyKind::Http, 8080, "HTTP_PROXY")
        );
        let https = select(&url("https://a.test/"), &ProxyMode::Environment, &vars)
            .unwrap()
            .unwrap();
        assert_eq!((https.kind, https.port), (ProxyKind::Socks5h, 1080));
    }

    #[test]
    fn all_proxy_is_the_fallback() {
        let p = select(
            &url("https://a.test/"),
            &ProxyMode::Environment,
            env(&[("ALL_PROXY", "socks5://s:9050")]),
        )
        .unwrap()
        .unwrap();
        assert_eq!(p.kind, ProxyKind::Socks5);
    }

    #[test]
    fn no_proxy_and_loopback_bypass_environment_proxies() {
        let vars = env(&[
            ("HTTP_PROXY", "proxy:3128"),
            (
                "NO_PROXY",
                "internal.test, .corp.test,10.0.0.5,example.org:8080",
            ),
        ]);
        let pick = |u: &str| select(&url(u), &ProxyMode::Environment, &vars).unwrap();
        assert!(pick("http://internal.test/").is_none());
        assert!(pick("http://api.internal.test/").is_none());
        assert!(pick("http://x.corp.test/").is_none());
        assert!(pick("http://10.0.0.5/").is_none());
        assert!(pick("http://example.org/").is_none());
        assert!(pick("http://notinternal.test/").is_some());
        assert!(pick("http://localhost:8080/").is_none());
        assert!(pick("http://127.0.0.1/").is_none());
        assert!(pick("http://[::1]/").is_none());

        // An explicit proxy is used even for loopback.
        let explicit = ProxyMode::Url("http://proxy:3128".into());
        assert!(
            select(&url("http://127.0.0.1/"), &explicit, env(&[]))
                .unwrap()
                .is_some()
        );
        assert!(
            select(&url("http://a.test/"), &ProxyMode::Direct, &vars)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn wildcard_no_proxy() {
        let vars = env(&[("HTTP_PROXY", "proxy:3128"), ("no_proxy", "*")]);
        assert!(
            select(&url("http://a.test/"), &ProxyMode::Environment, vars)
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn bad_proxy_urls() {
        let err = select(
            &url("http://a.test/"),
            &ProxyMode::Environment,
            env(&[("HTTP_PROXY", "ftp://p")]),
        )
        .unwrap_err();
        assert!(
            err.contains("unsupported proxy scheme `ftp`") && err.contains("HTTP_PROXY"),
            "{err}"
        );
    }
}
