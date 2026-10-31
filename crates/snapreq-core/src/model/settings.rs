//! Per-request execution settings.

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
    /// Verify the server's TLS certificate. Turning this off is insecure.
    pub verify_tls: bool,
    /// Which HTTP version to use.
    pub http_version: HttpVersionPref,
    /// Transparently decompress gzip/br/zstd/deflate responses.
    pub decompress: bool,
}

impl Default for RequestSettings {
    fn default() -> Self {
        Self {
            connect_timeout: Some(Duration::from_secs(10)),
            timeout: Some(Duration::from_secs(30)),
            follow_redirects: true,
            max_redirects: 10,
            verify_tls: true,
            http_version: HttpVersionPref::Auto,
            decompress: true,
        }
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
            serde_json::from_value(json!({ "timeout_ms": 1500.5, "verify_tls": false })).unwrap();
        assert_eq!(s.timeout, Some(Duration::from_micros(1_500_500)));
        assert!(!s.verify_tls);
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
