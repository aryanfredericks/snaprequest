//! The HTTP engine: executes a [`Request`] and returns a [`Response`] with a full trace.

mod build;
mod conn;
mod meter;
mod recorder;

use std::sync::Arc;
use std::time::{Duration, Instant};

use bytes::BytesMut;
use http::header;
use http_body_util::BodyExt;

use self::build::Prepared;
use self::conn::{Conn, Pool, PoolKey};
use self::recorder::Recorder;
use crate::model::{Header, HttpVersion, Phase, Request, Response, Sizes, Timings, TraceEvent};
use crate::{Error, Result};

/// Client-wide configuration. Per-request options live in
/// [`RequestSettings`](crate::model::RequestSettings).
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Default `User-Agent`, used when a request does not set one.
    pub user_agent: String,
    /// How long an idle connection is kept for reuse.
    pub pool_idle_timeout: Duration,
    /// Maximum idle connections kept per scheme/host/port. `0` disables reuse.
    pub pool_max_idle_per_host: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            user_agent: format!("snapreq/{}", crate::VERSION),
            pool_idle_timeout: Duration::from_secs(90),
            pool_max_idle_per_host: 8,
        }
    }
}

/// Executes requests. Cheap to clone; clones share the connection pool.
///
/// Must be used from inside a Tokio runtime.
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
    config: ClientConfig,
    pool: Pool,
}

impl Default for Client {
    fn default() -> Self {
        Self::new(ClientConfig::default())
    }
}

// Created once per request and immediately destructured; boxing would only add an allocation.
#[allow(clippy::large_enum_variant)]
enum Outcome {
    /// The response, the connection, and whether it may be reused.
    Done(Response, Conn, bool),
    /// A pooled connection turned out to be closed before the server sent anything.
    Stale(String),
}

impl Client {
    pub fn new(config: ClientConfig) -> Self {
        let pool = Pool::new(config.pool_idle_timeout, config.pool_max_idle_per_host);
        Self {
            inner: Arc::new(Inner { config, pool }),
        }
    }

    pub fn config(&self) -> &ClientConfig {
        &self.inner.config
    }

    /// Sends `req` and reads the whole response.
    ///
    /// On failure during execution the error is [`Error::Execution`], which carries
    /// the trace recorded up to the failure.
    pub async fn execute(&self, req: &Request) -> Result<Response> {
        let url = req.effective_url()?;
        if url.scheme() == "https" {
            return Err(Error::Unsupported("HTTPS".into()));
        }
        let prepared = build::prepare(req, &url, &self.inner.config.user_agent)?;

        let mut rec = Recorder::new();
        match req.settings.timeout {
            Some(limit) => {
                match tokio::time::timeout(limit, self.run(req, &url, &prepared, &mut rec)).await {
                    Ok(result) => result,
                    Err(_) => Err(rec.fail(
                        format!("request did not complete within {} ms", limit.as_millis()),
                        true,
                    )),
                }
            }
            None => self.run(req, &url, &prepared, &mut rec).await,
        }
    }

    async fn run(
        &self,
        req: &Request,
        url: &url::Url,
        prepared: &Prepared,
        rec: &mut Recorder,
    ) -> Result<Response> {
        let key = PoolKey::new(url);

        if let Some(conn) = self.reusable_conn(&key).await {
            rec.record(TraceEvent::ConnectionReused { addr: conn.remote });
            match self.exchange(conn, url, prepared, rec, true).await? {
                Outcome::Done(resp, conn, reusable) => {
                    return Ok(self.finish(key, resp, conn, reusable, rec));
                }
                Outcome::Stale(why) => rec.record(TraceEvent::Info {
                    message: format!(
                        "reused connection was closed by the server ({why}); retrying on a new connection"
                    ),
                }),
            }
        }

        let conn = self.connect(url, req, rec).await?;
        match self.exchange(conn, url, prepared, rec, false).await? {
            Outcome::Done(resp, conn, reusable) => Ok(self.finish(key, resp, conn, reusable, rec)),
            Outcome::Stale(why) => Err(rec.fail(why, false)),
        }
    }

    async fn reusable_conn(&self, key: &PoolKey) -> Option<Conn> {
        while let Some(mut conn) = self.inner.pool.checkout(key) {
            if conn.sender.ready().await.is_ok() {
                return Some(conn);
            }
        }
        None
    }

    async fn connect(&self, url: &url::Url, req: &Request, rec: &mut Recorder) -> Result<Conn> {
        match req.settings.connect_timeout {
            Some(limit) => match tokio::time::timeout(limit, conn::connect(url, rec)).await {
                Ok(result) => result,
                Err(_) => Err(rec.fail(
                    format!("no connection within {} ms", limit.as_millis()),
                    true,
                )),
            },
            None => conn::connect(url, rec).await,
        }
    }

    /// Sends the request on `conn` and reads the full response.
    async fn exchange(
        &self,
        mut conn: Conn,
        url: &url::Url,
        prepared: &Prepared,
        rec: &mut Recorder,
        reused: bool,
    ) -> Result<Outcome> {
        conn.meter.reset();
        rec.record(TraceEvent::RequestHeaders {
            method: prepared.to_http().method().to_string(),
            url: url.to_string(),
            version: HttpVersion::Http11,
            headers: prepared.sent_headers.clone(),
        });
        rec.begin_send(conn.meter.clone(), prepared.body_len);

        let response = match conn.sender.send_request(prepared.to_http()).await {
            Ok(r) => r,
            Err(e) => {
                let nothing_received = conn.meter.snapshot().bytes_read == 0;
                if reused
                    && nothing_received
                    && (e.is_canceled() || e.is_closed() || e.is_incomplete_message())
                {
                    rec.finish_send();
                    return Ok(Outcome::Stale(error_chain(&e)));
                }
                return Err(rec.fail(error_chain(&e), false));
            }
        };

        let first_byte = rec.finish_send().unwrap_or_else(Instant::now);
        let (parts, mut body) = response.into_parts();
        let version = HttpVersion::try_from(parts.version)?;
        let headers: Vec<Header> = parts
            .headers
            .iter()
            .map(|(n, v)| Header::new(n.as_str(), String::from_utf8_lossy(v.as_bytes())))
            .collect();
        rec.record(TraceEvent::ResponseHeaders {
            status: parts.status.as_u16(),
            version,
            headers: headers.clone(),
        });

        rec.phase = Phase::Download;
        let mut buf = BytesMut::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|e| rec.fail(error_chain(&e), false))?;
            if let Ok(data) = frame.into_data() {
                buf.extend_from_slice(&data);
            }
        }
        let body_len = buf.len() as u64;
        rec.record(TraceEvent::ResponseComplete {
            body_bytes: body_len,
            download: first_byte.elapsed(),
        });

        let snap = conn.meter.snapshot();
        let reusable = !parts
            .headers
            .get_all(header::CONNECTION)
            .iter()
            .any(|v| v.as_bytes().eq_ignore_ascii_case(b"close"));

        Ok(Outcome::Done(
            Response {
                status: parts.status.as_u16(),
                version,
                url: url.to_string(),
                headers,
                body: buf.freeze(),
                remote_addr: Some(conn.remote),
                timings: Timings::default(),
                sizes: Sizes {
                    request_headers: snap.bytes_written.saturating_sub(prepared.body_len),
                    request_body: prepared.body_len,
                    response_headers: snap.bytes_read.saturating_sub(body_len),
                    response_body: body_len,
                    response_body_decoded: body_len,
                },
                trace: Vec::new(),
            },
            conn,
            reusable,
        ))
    }

    /// Returns the connection to the pool (if reusable) and attaches trace and timings.
    fn finish(
        &self,
        key: PoolKey,
        mut resp: Response,
        conn: Conn,
        reusable: bool,
        rec: &mut Recorder,
    ) -> Response {
        if reusable {
            self.inner.pool.checkin(key, conn);
        }
        // Dropping a non-reusable `conn` here lets hyper close the connection.
        resp.trace = rec.take_entries();
        resp.timings = Timings::from_trace(&resp.trace);
        resp
    }
}

/// Formats an error with its chain of causes: `outer: inner: root`.
pub(crate) fn error_chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut source = e.source();
    while let Some(s) = source {
        let msg = s.to_string();
        if !out.ends_with(&msg) {
            out.push_str(": ");
            out.push_str(&msg);
        }
        source = s.source();
    }
    out
}
