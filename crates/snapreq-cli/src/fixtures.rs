//! Deterministic sample data for snapshot tests: a redirected HTTPS exchange with a
//! JSON response, and a failed one.

use std::time::Duration;

use bytes::Bytes;
use snapreq_core::model::{
    CertificateInfo, Header, HttpVersion, Phase, Sizes, Timings, TlsInfo, TraceEntry, TraceEvent,
};
use snapreq_core::{Request, Response};

fn at(us: u64, event: TraceEvent) -> TraceEntry {
    TraceEntry {
        at: Duration::from_micros(us),
        event,
    }
}

fn ms(n: u64) -> Duration {
    Duration::from_millis(n)
}

pub(crate) fn request() -> Request {
    Request::get("https://api.example.test/v1/items?page=2")
        .header("Authorization", "Bearer s3cr3t-token")
}

fn certificate() -> CertificateInfo {
    CertificateInfo {
        subject: "CN=api.example.test".into(),
        issuer: "C=US, O=Example CA, CN=Example Issuing CA".into(),
        not_before: "2026-01-01T00:00:00Z".into(),
        not_after: "2027-01-01T00:00:00Z".into(),
        serial: "0a:1b:2c".into(),
        subject_alt_names: vec!["api.example.test".into(), "*.example.test".into()],
        sha256_fingerprint: "AB:CD:EF".into(),
    }
}

pub(crate) fn trace() -> Vec<TraceEntry> {
    let addr = "203.0.113.10:443".parse().unwrap();
    let request_headers = |path: &str| TraceEvent::RequestHeaders {
        method: "GET".into(),
        url: format!("https://api.example.test{path}"),
        version: HttpVersion::Http11,
        headers: vec![
            Header::new("Host", "api.example.test"),
            Header::new("Authorization", "Bearer <redacted>"),
            Header::new("User-Agent", "snapreq/0.1.0"),
        ],
    };
    vec![
        at(10, TraceEvent::DnsStart { host: "api.example.test".into() }),
        at(4_010, TraceEvent::DnsEnd {
            host: "api.example.test".into(),
            addrs: vec!["203.0.113.10".parse().unwrap()],
            duration: ms(4),
        }),
        at(4_020, TraceEvent::ConnectStart { addr }),
        at(9_020, TraceEvent::ConnectEnd {
            addr,
            local_addr: "192.0.2.7:50123".parse().unwrap(),
            duration: ms(5),
        }),
        at(9_030, TraceEvent::TlsStart { server_name: "api.example.test".into() }),
        at(19_030, TraceEvent::TlsEnd {
            info: TlsInfo {
                version: "TLSv1.3".into(),
                cipher: "TLS13_AES_128_GCM_SHA256".into(),
                alpn: Some("http/1.1".into()),
                peer_certificates: vec![certificate()],
            },
            duration: ms(10),
        }),
        at(19_040, request_headers("/v1/items?page=2")),
        at(19_540, TraceEvent::RequestSent { body_bytes: 0, duration: Duration::from_micros(500) }),
        at(39_540, TraceEvent::ResponseFirstByte { wait: ms(20) }),
        at(39_600, TraceEvent::ResponseHeaders {
            status: 301,
            version: HttpVersion::Http11,
            headers: vec![Header::new("location", "/v2/items?page=2")],
        }),
        at(40_000, TraceEvent::ResponseComplete { body_bytes: 0, download: Duration::from_micros(400) }),
        at(40_010, TraceEvent::Redirect {
            status: 301,
            from: "https://api.example.test/v1/items?page=2".into(),
            to: "https://api.example.test/v2/items?page=2".into(),
        }),
        at(40_020, TraceEvent::ConnectionReused { addr }),
        at(40_030, request_headers("/v2/items?page=2")),
        at(40_330, TraceEvent::RequestSent { body_bytes: 0, duration: Duration::from_micros(300) }),
        at(70_330, TraceEvent::ResponseFirstByte { wait: ms(30) }),
        at(70_400, TraceEvent::ResponseHeaders {
            status: 200,
            version: HttpVersion::Http11,
            headers: vec![
                Header::new("content-type", "application/json"),
                Header::new("set-cookie", "sid=<redacted>; Path=/; HttpOnly"),
            ],
        }),
        at(72_330, TraceEvent::ResponseComplete { body_bytes: 56, download: Duration::from_micros(2_000) }),
        at(72_340, TraceEvent::Info { message: "stored cookie `sid`".into() }),
        at(72_350, TraceEvent::Wire {
            sent: Bytes::from_static(
                b"GET /v2/items?page=2 HTTP/1.1\r\nHost: api.example.test\r\nAuthorization: Bearer <redacted>\r\n\r\n",
            ),
            received: Bytes::from_static(
                b"HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 56\r\n\r\n{\"items\":[{\"id\":1,\"name\":\"snap\",\"tags\":[]}],\"next\":null}",
            ),
            truncated: false,
        }),
    ]
}

pub(crate) fn response() -> Response {
    let trace = trace();
    let body = Bytes::from_static(br#"{"items":[{"id":1,"name":"snap","tags":[]}],"next":null}"#);
    Response {
        status: 200,
        version: HttpVersion::Http11,
        url: "https://api.example.test/v2/items?page=2".into(),
        headers: vec![
            Header::new("content-type", "application/json"),
            Header::new("set-cookie", "sid=abc123; Path=/; HttpOnly"),
        ],
        remote_addr: Some("203.0.113.10:443".parse().unwrap()),
        timings: Timings::from_trace(&trace),
        sizes: Sizes {
            request_headers: 96,
            request_body: 0,
            response_headers: 88,
            response_body: body.len() as u64,
            response_body_decoded: body.len() as u64,
        },
        body,
        trace,
    }
}

pub(crate) fn connect_failure() -> snapreq_core::Error {
    snapreq_core::Error::Execution(Box::new(snapreq_core::ExecutionError {
        phase: Phase::Connect,
        timed_out: false,
        message: "could not connect to api.example.test:443: Connection refused".into(),
        trace: vec![
            at(
                10,
                TraceEvent::DnsStart {
                    host: "api.example.test".into(),
                },
            ),
            at(
                4_010,
                TraceEvent::DnsEnd {
                    host: "api.example.test".into(),
                    addrs: vec!["203.0.113.10".parse().unwrap()],
                    duration: ms(4),
                },
            ),
            at(
                4_020,
                TraceEvent::ConnectStart {
                    addr: "203.0.113.10:443".parse().unwrap(),
                },
            ),
            at(
                4_120,
                TraceEvent::Info {
                    message: "connect to 203.0.113.10:443 failed after 0 ms: Connection refused"
                        .into(),
                },
            ),
            at(
                4_130,
                TraceEvent::Error {
                    phase: Phase::Connect,
                    message: "could not connect to api.example.test:443: Connection refused".into(),
                },
            ),
        ],
    }))
}
