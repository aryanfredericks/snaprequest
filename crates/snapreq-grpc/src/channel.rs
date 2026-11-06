//! An HTTP/2 connection for gRPC, opened by the engine.

use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::{Duration, Instant};

use hyper::client::conn::http2::SendRequest;
use hyper_util::rt::{TokioExecutor, TokioIo};
use snapreq_core::model::{TraceEntry, TraceEvent};
use snapreq_core::{Client, Request, TraceSink};

use crate::{Error, Result};

/// `grpc://` → `http://`, `grpcs://` → `https://`; `host:port` → `http://host:port`.
pub fn http_url(url: &str) -> String {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if let Some(rest) = lower.strip_prefix("grpcs://").map(|_| &url[8..]) {
        format!("https://{rest}")
    } else if let Some(rest) = lower.strip_prefix("grpc://").map(|_| &url[7..]) {
        format!("http://{rest}")
    } else if url.contains("://") {
        url.to_owned()
    } else {
        format!("http://{url}")
    }
}

/// An open gRPC connection and what every call on it sends.
#[derive(Debug, Clone)]
pub struct Channel {
    pub(crate) send: H2,
    pub(crate) origin: http::Uri,
    /// Headers of the request plus its auth header, sent as metadata.
    pub metadata: Vec<(String, String)>,
    /// The request's total time limit, sent as `grpc-timeout` and enforced.
    pub timeout: Option<Duration>,
    /// DNS, connect and TLS events.
    pub trace: Vec<TraceEntry>,
    pub started: Instant,
    pub remote: std::net::SocketAddr,
    /// Values hidden wherever calls are shown.
    pub secrets: Vec<String>,
}

impl Channel {
    /// A trace entry for now.
    pub fn event(&self, event: TraceEvent) -> TraceEntry {
        TraceEntry {
            at: self.started.elapsed(),
            event,
        }
    }
}

/// hyper's HTTP/2 client as the service tonic sends on.
#[derive(Debug, Clone)]
pub(crate) struct H2(SendRequest<tonic::body::Body>);

impl tower_service::Service<http::Request<tonic::body::Body>> for H2 {
    type Response = http::Response<hyper::body::Incoming>;
    type Error = hyper::Error;
    type Future =
        Pin<Box<dyn Future<Output = Result<Self::Response, Self::Error>> + Send + 'static>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.0.poll_ready(cx)
    }

    fn call(&mut self, req: http::Request<tonic::body::Body>) -> Self::Future {
        let mut send = self.0.clone();
        Box::pin(async move { send.send_request(req).await })
    }
}

/// Connects to the host of `req.url` (`grpc://`, `grpcs://`, `http(s)://` or `host:port`).
pub async fn connect(client: &Client, req: &Request, sink: Option<TraceSink>) -> Result<Channel> {
    let mut req = req.clone();
    req.url = http_url(&req.url);
    let url = req
        .effective_url()
        .map_err(|e| Error::Invalid(e.to_string()))?;
    let mut metadata: Vec<(String, String)> = req
        .enabled_headers()
        .map(|h| (h.name.trim().to_ascii_lowercase(), h.value.clone()))
        .collect();
    if let Some((name, value)) = client.auth_header(&req).await? {
        metadata.push((name.to_ascii_lowercase(), value));
    }
    let stream = client.open_h2_stream(&req, sink).await?;
    let (send, connection) = hyper::client::conn::http2::Builder::new(TokioExecutor::new())
        .handshake(TokioIo::new(stream.io))
        .await
        .map_err(|e| Error::Invalid(format!("HTTP/2 handshake failed: {e}")))?;
    let remote = stream.remote;
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::debug!(error = %e, %remote, "gRPC connection closed with error");
        }
    });
    let origin: http::Uri = format!(
        "{}://{}",
        url.scheme(),
        &url[url::Position::BeforeHost..url::Position::AfterPort]
    )
    .parse()
    .map_err(|e| Error::Invalid(format!("bad origin: {e}")))?;
    Ok(Channel {
        send: H2(send),
        origin,
        metadata,
        timeout: req.settings.timeout,
        trace: stream.trace,
        started: stream.started,
        remote,
        secrets: req.secrets(),
    })
}
