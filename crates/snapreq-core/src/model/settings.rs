//! Per-request execution settings.

use std::net::IpAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How a request is executed. Every field has a sensible default, so a
/// partial JSON/TOML object is valid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RequestSettings {
    /// Maximum time to establish the connection (DNS + TCP + TLS). `None` means no limit.
    #[serde(
        with = "crate::serde_util::opt_duration_ms",
        rename = "connect_timeout_ms"
    )]
    pub connect_timeout: Option<Duration>,
    /// Maximum time for the whole exchange, including redirects. `None` means no limit.
    #[serde(with = "crate::serde_util::opt_duration_ms", rename = "timeout_ms")]
    pub timeout: Option<Duration>,
    /// Follow `3xx` redirects automatically.
    pub follow_redirects: bool,
    /// Maximum number of redirects to follow.
    pub max_redirects: u32,
    /// TLS options (only used for `https` URLs).
    pub tls: TlsSettings,
    /// Which HTTP version to use.
    pub http_version: HttpVersionPref,
    /// Transparently decompress gzip/br/zstd/deflate responses.
    pub decompress: bool,
    /// Hosts pinned to fixed addresses instead of DNS, like curl's `--resolve`.
    /// TLS still verifies against the host name from the URL.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub resolve: Vec<ResolveOverride>,
}

impl Default for RequestSettings {
    fn default() -> Self {
        Self {
            connect_timeout: Some(Duration::from_secs(10)),
            timeout: Some(Duration::from_secs(30)),
            follow_redirects: true,
            max_redirects: 10,
            tls: TlsSettings::default(),
            http_version: HttpVersionPref::Auto,
            decompress: true,
            resolve: Vec::new(),
        }
    }
}

/// TLS options for one request.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct TlsSettings {
    /// Verify the server's certificate chain and host name. Turning this off is insecure;
    /// the trace still reports what verification would have said.
    pub verify: bool,
    /// PEM file with extra CA certificates to trust, in addition to the system store.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<PathBuf>,
    /// Client certificate for mutual TLS.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<ClientCertificate>,
    /// Lowest TLS version to allow. `None` means TLS 1.2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_version: Option<TlsVersion>,
    /// Highest TLS version to allow. `None` means TLS 1.3.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_version: Option<TlsVersion>,
}

impl Default for TlsSettings {
    fn default() -> Self {
        Self {
            verify: true,
            ca_cert: None,
            client_cert: None,
            min_version: None,
            max_version: None,
        }
    }
}

impl TlsSettings {
    /// `true` when nothing differs from the defaults.
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// A client certificate chain and its private key, both PEM files.
/// They may be the same file if it contains both.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ClientCertificate {
    pub cert: PathBuf,
    pub key: PathBuf,
}

/// A TLS protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TlsVersion {
    #[serde(rename = "1.2")]
    Tls12,
    #[serde(rename = "1.3")]
    Tls13,
}

/// Pins `host` (optionally only on `port`) to fixed addresses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolveOverride {
    pub host: String,
    /// Only apply on this port; `None` applies to every port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub addrs: Vec<IpAddr>,
}

impl ResolveOverride {
    /// Whether this override applies to `host:port` (host compared case-insensitively).
    pub fn matches(&self, host: &str, port: u16) -> bool {
        self.host.eq_ignore_ascii_case(host) && self.port.is_none_or(|p| p == port)
    }
}

/// Parses curl's `--resolve` syntax: `host:port:addr[,addr...]`, where `port` may be `*`
/// and IPv6 addresses may be written in brackets (`[::1]`).
impl FromStr for ResolveOverride {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let usage = || format!("invalid resolve entry `{s}`, expected HOST:PORT:ADDR[,ADDR...]");
        let (host, rest) = s.split_once(':').ok_or_else(usage)?;
        let (port, addrs) = rest.split_once(':').ok_or_else(usage)?;
        if host.is_empty() || addrs.is_empty() {
            return Err(usage());
        }
        let port = match port {
            "*" => None,
            p => Some(
                p.parse::<u16>()
                    .map_err(|_| format!("invalid port `{p}` in `{s}`"))?,
            ),
        };
        let addrs = addrs
            .split(',')
            .map(|a| {
                let a = a.trim();
                let bare = a
                    .strip_prefix('[')
                    .and_then(|a| a.strip_suffix(']'))
                    .unwrap_or(a);
                bare.parse::<IpAddr>()
                    .map_err(|_| format!("invalid IP address `{a}` in `{s}`"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            host: host.to_owned(),
            port,
            addrs,
        })
    }
}

/// Requested HTTP version.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HttpVersionPref {
    /// HTTP/2 when the server offers it through ALPN, otherwise HTTP/1.1.
    #[default]
    Auto,
    /// Always HTTP/1.1.
    Http1,
    /// HTTP/2 only (over TLS via ALPN, or prior knowledge over plain TCP).
    Http2,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn partial_settings_fill_in_defaults() {
        let s: RequestSettings =
            serde_json::from_value(json!({ "timeout_ms": 1500.5, "tls": { "verify": false } }))
                .unwrap();
        assert_eq!(s.timeout, Some(Duration::from_micros(1_500_500)));
        assert!(!s.tls.verify);
        assert_eq!(s.tls.min_version, None);
        assert_eq!(s.max_redirects, RequestSettings::default().max_redirects);
    }

    #[test]
    fn null_timeout_means_no_limit() {
        let s: RequestSettings = serde_json::from_value(json!({ "timeout_ms": null })).unwrap();
        assert_eq!(s.timeout, None);
    }

    #[test]
    fn negative_timeout_is_rejected() {
        let err = serde_json::from_value::<RequestSettings>(json!({ "timeout_ms": -1 }))
            .unwrap_err()
            .to_string();
        assert!(err.contains("invalid duration"), "{err}");
    }

    #[test]
    fn resolve_override_parsing() {
        let r: ResolveOverride = "api.test:443:127.0.0.1,[::1]".parse().unwrap();
        assert_eq!(r.host, "api.test");
        assert_eq!(r.port, Some(443));
        assert_eq!(
            r.addrs,
            [
                "127.0.0.1".parse::<IpAddr>().unwrap(),
                "::1".parse().unwrap()
            ]
        );
        assert!(r.matches("API.test", 443));
        assert!(!r.matches("api.test", 80));

        let any: ResolveOverride = "x.test:*:10.0.0.1".parse().unwrap();
        assert!(any.matches("x.test", 8080));

        for bad in [
            "nohost",
            "h:80",
            "h:80:",
            "h:x:1.2.3.4",
            "h:80:not-an-ip",
            ":80:1.2.3.4",
        ] {
            assert!(
                bad.parse::<ResolveOverride>().is_err(),
                "`{bad}` should not parse"
            );
        }
    }

    #[test]
    fn tls_settings_json() {
        let tls: TlsSettings = serde_json::from_value(json!({
            "min_version": "1.2",
            "max_version": "1.3",
            "client_cert": { "cert": "me.crt", "key": "me.key" }
        }))
        .unwrap();
        assert!(tls.verify);
        assert_eq!(tls.min_version, Some(TlsVersion::Tls12));
        assert!(TlsVersion::Tls12 < TlsVersion::Tls13);
        assert!(!tls.is_default());
        assert!(TlsSettings::default().is_default());
        assert!(serde_json::from_value::<TlsSettings>(json!({ "min_version": "1.1" })).is_err());
    }

    #[test]
    fn round_trip() {
        let s = RequestSettings {
            http_version: HttpVersionPref::Http2,
            ..Default::default()
        };
        let back: RequestSettings =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
