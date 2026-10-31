//! The result of executing a request.

use std::fmt;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use serde::{Deserialize, Serialize};

use super::{TraceEntry, TraceEvent};

/// A received response together with its timings and full trace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Response {
    pub status: u16,
    pub version: HttpVersion,
    /// Final URL, after redirects.
    pub url: String,
    pub headers: Vec<Header>,
    /// Body after decompression (if enabled).
    #[serde(with = "crate::serde_util::raw_bytes")]
    pub body: Bytes,
    /// Address of the server that answered the final hop.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_addr: Option<SocketAddr>,
    pub timings: Timings,
    pub sizes: Sizes,
    pub trace: Vec<TraceEntry>,
}

impl Response {
    /// Canonical reason phrase for the status (`"Not Found"`), if one exists.
    pub fn status_text(&self) -> Option<&'static str> {
        http::StatusCode::from_u16(self.status)
            .ok()
            .and_then(|s| s.canonical_reason())
    }

    /// `true` for 2xx statuses.
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// First header with this name (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case(name))
            .map(|h| h.value.as_str())
    }

    /// The body as text, if it is valid UTF-8.
    pub fn text(&self) -> Option<&str> {
        std::str::from_utf8(&self.body).ok()
    }
}

/// One header line. Values that are not valid UTF-8 are converted lossily here;
/// the exact bytes are available in the wire capture.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

impl Header {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
        }
    }
}

/// HTTP protocol version.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HttpVersion {
    #[serde(rename = "HTTP/0.9")]
    Http09,
    #[serde(rename = "HTTP/1.0")]
    Http10,
    #[serde(rename = "HTTP/1.1")]
    Http11,
    #[serde(rename = "HTTP/2")]
    Http2,
    #[serde(rename = "HTTP/3")]
    Http3,
}

impl HttpVersion {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Http09 => "HTTP/0.9",
            Self::Http10 => "HTTP/1.0",
            Self::Http11 => "HTTP/1.1",
            Self::Http2 => "HTTP/2",
            Self::Http3 => "HTTP/3",
        }
    }
}

impl fmt::Display for HttpVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl TryFrom<http::Version> for HttpVersion {
    type Error = crate::Error;

    fn try_from(v: http::Version) -> crate::Result<Self> {
        Ok(match v {
            http::Version::HTTP_09 => Self::Http09,
            http::Version::HTTP_10 => Self::Http10,
            http::Version::HTTP_11 => Self::Http11,
            http::Version::HTTP_2 => Self::Http2,
            http::Version::HTTP_3 => Self::Http3,
            other => return Err(crate::Error::UnsupportedVersion(format!("{other:?}"))),
        })
    }
}

/// Time spent in each phase of the **final** hop, plus the overall total.
///
/// A phase is `None` when it did not happen, e.g. no DNS or TLS on a reused connection.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timings {
    #[serde(with = "crate::serde_util::opt_duration_ms", rename = "dns_ms")]
    pub dns: Option<Duration>,
    #[serde(with = "crate::serde_util::opt_duration_ms", rename = "connect_ms")]
    pub connect: Option<Duration>,
    #[serde(with = "crate::serde_util::opt_duration_ms", rename = "tls_ms")]
    pub tls: Option<Duration>,
    /// Writing the request.
    #[serde(with = "crate::serde_util::opt_duration_ms", rename = "send_ms")]
    pub send: Option<Duration>,
    /// Waiting for the first response byte (time to first byte after sending).
    #[serde(with = "crate::serde_util::opt_duration_ms", rename = "wait_ms")]
    pub wait: Option<Duration>,
    /// Receiving the response body.
    #[serde(with = "crate::serde_util::opt_duration_ms", rename = "download_ms")]
    pub download: Option<Duration>,
    /// Whole execution, including all redirects.
    #[serde(with = "crate::serde_util::duration_ms", rename = "total_ms")]
    pub total: Duration,
}

impl Timings {
    /// Derives the timings from a trace, so the two can never disagree.
    ///
    /// Phases are taken from the events after the last [`TraceEvent::Redirect`];
    /// `total` is the timestamp of the last event.
    pub fn from_trace(trace: &[TraceEntry]) -> Self {
        let last_hop_start = trace
            .iter()
            .rposition(|e| matches!(e.event, TraceEvent::Redirect { .. }))
            .map_or(0, |i| i + 1);

        let mut t = Self {
            total: trace.last().map(|e| e.at).unwrap_or_default(),
            ..Self::default()
        };
        for entry in &trace[last_hop_start..] {
            match &entry.event {
                TraceEvent::DnsEnd { duration, .. } => t.dns = Some(*duration),
                TraceEvent::ConnectEnd { duration, .. } => t.connect = Some(*duration),
                TraceEvent::TlsEnd { duration, .. } => t.tls = Some(*duration),
                TraceEvent::RequestSent { duration, .. } => t.send = Some(*duration),
                TraceEvent::ResponseFirstByte { wait } => t.wait = Some(*wait),
                TraceEvent::ResponseComplete { download, .. } => t.download = Some(*download),
                _ => {}
            }
        }
        t
    }
}

/// Sizes in bytes.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sizes {
    /// Request headers as sent.
    pub request_headers: u64,
    pub request_body: u64,
    /// Response headers as received.
    pub response_headers: u64,
    /// Response body as received on the wire (possibly compressed).
    pub response_body: u64,
    /// Response body after decompression.
    pub response_body_decoded: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::TlsInfo;
    use serde_json::json;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    fn entry(at: u64, event: TraceEvent) -> TraceEntry {
        TraceEntry { at: ms(at), event }
    }

    fn sample_trace() -> Vec<TraceEntry> {
        let addr: SocketAddr = "93.184.216.34:443".parse().unwrap();
        vec![
            entry(
                0,
                TraceEvent::DnsStart {
                    host: "a.test".into(),
                },
            ),
            entry(
                5,
                TraceEvent::DnsEnd {
                    host: "a.test".into(),
                    addrs: vec![addr.ip()],
                    duration: ms(5),
                },
            ),
            entry(5, TraceEvent::ConnectStart { addr }),
            entry(
                25,
                TraceEvent::ConnectEnd {
                    addr,
                    local_addr: "10.0.0.2:50000".parse().unwrap(),
                    duration: ms(20),
                },
            ),
            entry(
                25,
                TraceEvent::TlsStart {
                    server_name: "a.test".into(),
                },
            ),
            entry(
                60,
                TraceEvent::TlsEnd {
                    info: TlsInfo {
                        version: "TLSv1.3".into(),
                        cipher: "TLS13_AES_128_GCM_SHA256".into(),
                        alpn: Some("h2".into()),
                        peer_certificates: vec![],
                    },
                    duration: ms(35),
                },
            ),
            entry(
                61,
                TraceEvent::RequestSent {
                    body_bytes: 0,
                    duration: ms(1),
                },
            ),
            entry(100, TraceEvent::ResponseFirstByte { wait: ms(39) }),
            entry(
                100,
                TraceEvent::ResponseHeaders {
                    status: 301,
                    version: HttpVersion::Http2,
                    headers: vec![Header::new("location", "/b")],
                },
            ),
            entry(
                101,
                TraceEvent::ResponseComplete {
                    body_bytes: 0,
                    download: ms(1),
                },
            ),
            entry(
                101,
                TraceEvent::Redirect {
                    status: 301,
                    from: "https://a.test/".into(),
                    to: "https://a.test/b".into(),
                },
            ),
            // Second hop reuses the connection: no DNS/connect/TLS.
            entry(101, TraceEvent::ConnectionReused { addr }),
            entry(
                102,
                TraceEvent::RequestSent {
                    body_bytes: 0,
                    duration: ms(1),
                },
            ),
            entry(130, TraceEvent::ResponseFirstByte { wait: ms(28) }),
            entry(
                150,
                TraceEvent::ResponseComplete {
                    body_bytes: 512,
                    download: ms(20),
                },
            ),
        ]
    }

    #[test]
    fn timings_come_from_the_final_hop() {
        let t = Timings::from_trace(&sample_trace());
        assert_eq!(
            t,
            Timings {
                dns: None,
                connect: None,
                tls: None,
                send: Some(ms(1)),
                wait: Some(ms(28)),
                download: Some(ms(20)),
                total: ms(150),
            }
        );
    }

    #[test]
    fn timings_without_redirects_include_every_phase() {
        let trace: Vec<_> = sample_trace().into_iter().take(10).collect();
        let t = Timings::from_trace(&trace);
        assert_eq!(t.dns, Some(ms(5)));
        assert_eq!(t.connect, Some(ms(20)));
        assert_eq!(t.tls, Some(ms(35)));
        assert_eq!(t.wait, Some(ms(39)));
        assert_eq!(t.total, ms(101));
    }

    #[test]
    fn empty_trace_gives_zero_total() {
        assert_eq!(Timings::from_trace(&[]), Timings::default());
    }

    #[test]
    fn trace_entry_json_shape() {
        let e = entry(
            5,
            TraceEvent::DnsEnd {
                host: "a.test".into(),
                addrs: vec!["::1".parse().unwrap()],
                duration: Duration::from_micros(4_321),
            },
        );
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            json!({
                "at_ms": 5.0, "event": "dns_end", "host": "a.test",
                "addrs": ["::1"], "duration_ms": 4.321
            })
        );
    }

    fn sample_response(body: &'static [u8]) -> Response {
        let trace = sample_trace();
        Response {
            status: 200,
            version: HttpVersion::Http2,
            url: "https://a.test/b".into(),
            headers: vec![Header::new("Content-Type", "application/json")],
            body: Bytes::from_static(body),
            remote_addr: Some("93.184.216.34:443".parse().unwrap()),
            timings: Timings::from_trace(&trace),
            sizes: Sizes {
                response_body: body.len() as u64,
                response_body_decoded: body.len() as u64,
                ..Sizes::default()
            },
            trace,
        }
    }

    #[test]
    fn response_round_trips_through_json() {
        for body in [&b"{\"ok\":true}"[..], &[0xff, 0x00, 0xfe][..]] {
            let resp = sample_response(body);
            let json = serde_json::to_string(&resp).unwrap();
            assert_eq!(serde_json::from_str::<Response>(&json).unwrap(), resp);
        }
    }

    #[test]
    fn body_encoding_in_json() {
        let text = serde_json::to_value(sample_response(b"hi")).unwrap();
        assert_eq!(text["body"], json!({ "encoding": "utf8", "data": "hi" }));
        let binary = serde_json::to_value(sample_response(&[0xff, 0x00])).unwrap();
        assert_eq!(
            binary["body"],
            json!({ "encoding": "base64", "data": "/wA=" })
        );
    }

    #[test]
    fn helpers() {
        let resp = sample_response(b"{}");
        assert_eq!(resp.header("content-type"), Some("application/json"));
        assert_eq!(resp.header("x-missing"), None);
        assert_eq!(resp.status_text(), Some("OK"));
        assert!(resp.is_success());
        assert_eq!(resp.text(), Some("{}"));
        assert_eq!(
            HttpVersion::try_from(http::Version::HTTP_11)
                .unwrap()
                .to_string(),
            "HTTP/1.1"
        );
    }
}
