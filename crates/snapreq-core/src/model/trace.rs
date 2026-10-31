//! The trace: an ordered record of everything that happened while executing a request.
//!
//! The CLI's verbose output, the server's stored logs, the web UI's timeline and test
//! reports are all rendered from this one structure.

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{Header, HttpVersion};

/// One trace event plus when it happened, relative to the start of the execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraceEntry {
    /// Time since the execution started.
    #[serde(with = "crate::serde_util::duration_ms", rename = "at_ms")]
    pub at: Duration,
    #[serde(flatten)]
    pub event: TraceEvent,
}

/// Something that happened during execution.
///
/// Serialized with an `event` tag, e.g. `{ "at_ms": 1.2, "event": "dns_start", "host": "example.com" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
#[non_exhaustive]
pub enum TraceEvent {
    /// DNS lookup started.
    DnsStart { host: String },
    /// DNS lookup finished.
    DnsEnd {
        host: String,
        addrs: Vec<IpAddr>,
        #[serde(with = "crate::serde_util::duration_ms", rename = "duration_ms")]
        duration: Duration,
    },
    /// TCP connection attempt started.
    ConnectStart { addr: SocketAddr },
    /// TCP connection established.
    ConnectEnd {
        addr: SocketAddr,
        local_addr: SocketAddr,
        #[serde(with = "crate::serde_util::duration_ms", rename = "duration_ms")]
        duration: Duration,
    },
    /// An idle pooled connection was reused; no DNS, connect or TLS happened.
    ConnectionReused { addr: SocketAddr },
    /// TLS handshake started.
    TlsStart { server_name: String },
    /// TLS handshake finished.
    TlsEnd {
        info: TlsInfo,
        #[serde(with = "crate::serde_util::duration_ms", rename = "duration_ms")]
        duration: Duration,
    },
    /// The request line and headers as they were sent.
    RequestHeaders {
        method: String,
        url: String,
        version: HttpVersion,
        headers: Vec<Header>,
    },
    /// The whole request (headers and body) has been written.
    RequestSent {
        body_bytes: u64,
        /// Time spent writing the request.
        #[serde(with = "crate::serde_util::duration_ms", rename = "duration_ms")]
        duration: Duration,
    },
    /// First byte of the response arrived.
    ResponseFirstByte {
        /// Time between the request being sent and the first response byte (server think time).
        #[serde(with = "crate::serde_util::duration_ms", rename = "wait_ms")]
        wait: Duration,
    },
    /// Response status line and headers.
    ResponseHeaders {
        status: u16,
        version: HttpVersion,
        headers: Vec<Header>,
    },
    /// Response body fully received.
    ResponseComplete {
        /// Body size as received on the wire (before decompression).
        body_bytes: u64,
        /// Time from the first response byte to the last.
        #[serde(with = "crate::serde_util::duration_ms", rename = "download_ms")]
        download: Duration,
    },
    /// Exact HTTP/1.1 bytes of one exchange (inside TLS), when wire capture is on.
    /// Each direction is capped; `truncated` says whether anything was cut.
    Wire {
        #[serde(with = "crate::serde_util::raw_bytes")]
        sent: bytes::Bytes,
        #[serde(with = "crate::serde_util::raw_bytes")]
        received: bytes::Bytes,
        truncated: bool,
    },
    /// A redirect is being followed. Events after this one belong to the next hop.
    Redirect {
        status: u16,
        from: String,
        to: String,
    },
    /// Free-form note (for example "decompressed gzip body").
    Info { message: String },
    /// Execution failed in the given phase.
    Error { phase: Phase, message: String },
}

/// Execution phases, used to say where an error happened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Prepare,
    Dns,
    Connect,
    Tls,
    Send,
    Wait,
    Download,
    Redirect,
}

impl Phase {
    /// Short human-readable description, used in error messages.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Prepare => "preparing request",
            Self::Dns => "DNS lookup",
            Self::Connect => "TCP connect",
            Self::Tls => "TLS handshake",
            Self::Send => "sending request",
            Self::Wait => "waiting for response",
            Self::Download => "downloading response",
            Self::Redirect => "following redirect",
        }
    }
}

impl std::fmt::Display for Phase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Details of a completed TLS handshake.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TlsInfo {
    /// Protocol version, e.g. `TLSv1.3`.
    pub version: String,
    /// Negotiated cipher suite, e.g. `TLS13_AES_128_GCM_SHA256`.
    pub cipher: String,
    /// Protocol chosen by ALPN, e.g. `h2` or `http/1.1`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alpn: Option<String>,
    /// Certificate chain presented by the server, leaf first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub peer_certificates: Vec<CertificateInfo>,
}

/// A summary of one X.509 certificate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CertificateInfo {
    pub subject: String,
    pub issuer: String,
    /// Validity start, RFC 3339.
    pub not_before: String,
    /// Validity end, RFC 3339.
    pub not_after: String,
    /// Serial number, hex.
    pub serial: String,
    /// Subject alternative names (DNS names and IPs).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subject_alt_names: Vec<String>,
    /// SHA-256 fingerprint of the DER encoding, hex.
    pub sha256_fingerprint: String,
}
