//! Connections for protocols built on HTTP/2 elsewhere (gRPC, PLAN P10.4): DNS, the outbound
//! policy, proxies (always tunnelled) and TLS as for any request, then the bare stream.

use std::net::SocketAddr;
use std::time::Instant;

use tokio::io::{AsyncRead, AsyncWrite};

use super::{Client, TraceSink, recorder};
use crate::model::{HttpVersionPref, Request, TraceEntry, TraceEvent};
use crate::{Error, Result};

/// A byte stream (TCP or TLS).
pub trait Io: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Io for T {}

/// An open stream to a request's host that speaks HTTP/2.
pub struct RawStream {
    pub io: Box<dyn Io>,
    pub remote: SocketAddr,
    /// DNS, connect and TLS events (redacted).
    pub trace: Vec<TraceEntry>,
    /// When the trace started (for adding later events).
    pub started: Instant,
}

impl std::fmt::Debug for RawStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawStream")
            .field("remote", &self.remote)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// Opens an HTTP/2 stream to the host of `req.url` (`http`: prior knowledge, `https`: TLS
    /// with ALPN `h2`), with the request's TLS, proxy and timeout settings. Nothing is sent.
    pub async fn open_h2_stream(
        &self,
        req: &Request,
        sink: Option<TraceSink>,
    ) -> Result<RawStream> {
        let url = req.effective_url()?;
        let mut req = req.clone();
        req.settings.http_version = HttpVersionPref::Http2;
        let mut rec = self.recorder(&req);
        rec.sink = sink.map(recorder::Sink);
        let started = Instant::now();
        let tls = match url.scheme() {
            "https" => Some(
                self.inner
                    .tls
                    .for_request(&req.settings.tls, HttpVersionPref::Http2)
                    .await?,
            ),
            _ => None,
        };
        let proxy = super::proxy::select(&url, &req.settings.proxy, |k| std::env::var(k).ok())
            .map_err(Error::InvalidRequest)?;
        if let Some((_, password)) = proxy.as_ref().and_then(|p| p.credentials.as_ref()) {
            rec.add_secret(password);
        }
        let work = super::conn::open(
            &url,
            &req.settings,
            tls.as_ref(),
            proxy.as_ref(),
            self.policy(),
            &mut rec,
            true,
        );
        let opened = match req.settings.connect_timeout {
            Some(limit) => match tokio::time::timeout(limit, work).await {
                Ok(r) => r?,
                Err(_) => {
                    return Err(rec.fail(
                        format!("no connection within {} ms", limit.as_millis()),
                        true,
                    ));
                }
            },
            None => work.await?,
        };
        if !opened.h2 {
            return Err(rec.fail("the connection does not use HTTP/2", false));
        }
        rec.record(TraceEvent::Info {
            message: "HTTP/2 connection open".into(),
        });
        Ok(RawStream {
            io: Box::new(opened.stream),
            remote: opened.remote,
            trace: rec.take_entries(),
            started,
        })
    }

    /// The header the request's auth adds when the engine does not send the request itself:
    /// basic, bearer and header API keys as they are, OAuth 2.0 with a token (cached or
    /// fetched). Digest and AWS Signature V4 need the HTTP exchange and are not supported.
    pub async fn auth_header(&self, req: &Request) -> Result<Option<(String, String)>> {
        use crate::model::Auth;
        if crate::auth::shadowed_by_header(req).is_some() {
            return Ok(None);
        }
        match &req.auth {
            Auth::OAuth2(o) => {
                let mut rec = self.recorder(req);
                let token = self.oauth_token(req, o, &mut rec, false).await?;
                Ok(Some(("Authorization".into(), token.header())))
            }
            Auth::Digest { .. } | Auth::AwsSigV4 { .. } => Err(Error::Unsupported(format!(
                "{} auth for this protocol",
                req.auth.kind()
            ))),
            Auth::ApiKey {
                location: crate::model::ApiKeyLocation::Query,
                ..
            } => Err(Error::Unsupported(
                "API keys in the query for this protocol".into(),
            )),
            other => Ok(crate::auth::header(other).map(|(n, v)| (n.to_owned(), v))),
        }
    }
}
