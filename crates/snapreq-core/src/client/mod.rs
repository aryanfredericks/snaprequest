//! The HTTP engine: executes a [`Request`] and returns a [`Response`] with a full trace.

mod body;
mod build;
mod conn;
mod cookies;
mod decode;
mod meter;
mod proxy;
mod recorder;
mod redact;
mod redirect;
mod tls;

pub use self::cookies::StoredCookie;
pub use self::redact::DEFAULT_SENSITIVE_HEADERS;
pub use self::tls::Certificate;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::BytesMut;
use http::header;
use http_body_util::BodyExt;

use self::build::Prepared;
use self::conn::{Conn, Pool, PoolKey};
use self::recorder::{Recorder, SendTiming};
use self::tls::{RequestTls, TlsContext};
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
    /// Extra trusted root certificates, added to the OS trust store
    /// (for example a company or local development CA).
    pub extra_root_certificates: Vec<Certificate>,
    /// Header names whose values are redacted in traces (case-insensitive).
    pub sensitive_headers: Vec<String>,
    /// Maximum bytes captured per direction when wire capture is on.
    pub wire_capture_limit: usize,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            user_agent: format!("snapreq/{}", crate::VERSION),
            pool_idle_timeout: Duration::from_secs(90),
            pool_max_idle_per_host: 8,
            extra_root_certificates: Vec::new(),
            sensitive_headers: DEFAULT_SENSITIVE_HEADERS
                .iter()
                .map(|h| (*h).to_owned())
                .collect(),
            wire_capture_limit: 1024 * 1024,
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
    tls: TlsContext,
    cookies: cookies::Jar,
}

impl Default for Client {
    /// A client with the default configuration (OS trust store, no extra roots).
    fn default() -> Self {
        let config = ClientConfig::default();
        let tls = TlsContext::new(&[])
            // Only extra roots can make this fail, and the default has none.
            .unwrap_or_else(|e| unreachable!("default TLS configuration is valid: {e}"));
        Self::from_parts(config, tls)
    }
}

/// One hop of a (possibly redirected) execution, ready to send.
struct Hop {
    prepared: Prepared,
    tls: Option<RequestTls>,
    proxy: Option<proxy::Proxy>,
    /// Names of the jar cookies included in this hop's `Cookie` header.
    jar_cookie_names: Vec<String>,
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
    /// Creates a client. Fails only if an extra root certificate is invalid.
    pub fn new(config: ClientConfig) -> Result<Self> {
        let tls = TlsContext::new(&config.extra_root_certificates)?;
        Ok(Self::from_parts(config, tls))
    }

    fn from_parts(config: ClientConfig, tls: TlsContext) -> Self {
        let pool = Pool::new(config.pool_idle_timeout, config.pool_max_idle_per_host);
        Self {
            inner: Arc::new(Inner {
                config,
                pool,
                tls,
                cookies: cookies::Jar::default(),
            }),
        }
    }

    pub fn config(&self) -> &ClientConfig {
        &self.inner.config
    }

    /// Unexpired cookies currently in this client's jar.
    pub fn cookies(&self) -> Vec<StoredCookie> {
        self.inner.cookies.list()
    }

    /// Empties this client's cookie jar.
    pub fn clear_cookies(&self) {
        self.inner.cookies.clear();
    }

    /// Sends `req`, follows redirects if enabled, and reads the whole response.
    ///
    /// Problems with the request itself (bad URL, unreadable file) are returned before
    /// any network activity. Failures during execution are [`Error::Execution`], which
    /// carries the trace recorded up to the failure.
    pub async fn execute(&self, req: &Request) -> Result<Response> {
        let url = req.effective_url()?;
        let first = self.prepare_hop(req, &url, None).await?;

        let mut rec = Recorder::new(redact::Redactor::new(
            &self.inner.config.sensitive_headers,
            &req.secrets(),
        ));
        if let Some(name) = crate::auth::shadowed_by_header(req) {
            rec.record(TraceEvent::Info {
                message: format!(
                    "{} auth not applied: the request sets the {name} header itself",
                    req.auth.kind()
                ),
            });
        }
        let result = match req.settings.timeout {
            Some(limit) => {
                match tokio::time::timeout(limit, self.follow(req, url, first, &mut rec)).await {
                    Ok(result) => result,
                    Err(_) => Err(rec.fail(
                        format!("request did not complete within {} ms", limit.as_millis()),
                        true,
                    )),
                }
            }
            None => self.follow(req, url, first, &mut rec).await,
        };
        let mut resp = result?;
        resp.trace = rec.take_entries();
        resp.timings = Timings::from_trace(&resp.trace);
        Ok(resp)
    }

    /// Encodes the body and picks the TLS configuration for one hop. `challenge_answer` is
    /// the `Authorization` value answering an auth challenge (Digest).
    async fn prepare_hop(
        &self,
        req: &Request,
        url: &url::Url,
        challenge_answer: Option<String>,
    ) -> Result<Hop> {
        let jar_cookies = if req.settings.cookies {
            self.inner.cookies.for_request(url)
        } else {
            Vec::new()
        };
        let auth_header = match challenge_answer {
            Some(value) => Some(("Authorization", value)),
            None => crate::auth::header(&req.auth),
        };
        let prepared = build::prepare(
            req,
            url,
            &self.inner.config.user_agent,
            &jar_cookies,
            auth_header,
        )
        .await?;
        let tls = match url.scheme() {
            "https" => Some(
                self.inner
                    .tls
                    .for_request(&req.settings.tls, req.settings.http_version)
                    .await?,
            ),
            _ => None,
        };
        let proxy = proxy::select(url, &req.settings.proxy, |k| std::env::var(k).ok())
            .map_err(Error::InvalidRequest)?;
        Ok(Hop {
            prepared,
            tls,
            proxy,
            jar_cookie_names: jar_cookies.into_iter().map(|(n, _)| n).collect(),
        })
    }

    /// Runs the first hop and then every redirect.
    async fn follow(
        &self,
        req: &Request,
        mut url: url::Url,
        mut hop: Hop,
        rec: &mut Recorder,
    ) -> Result<Response> {
        let mut current = req.clone();
        let mut followed = 0u32;
        // Whether the current hop has already answered an auth challenge.
        let mut answered = false;
        loop {
            if !hop.jar_cookie_names.is_empty() {
                rec.record(TraceEvent::Info {
                    message: format!(
                        "sending {} cookie(s) from the jar: {}",
                        hop.jar_cookie_names.len(),
                        hop.jar_cookie_names.join(", ")
                    ),
                });
            }
            // Credentials sent on this hop are hidden wherever they show up in the trace.
            rec.learn_secrets_from_headers(&hop.prepared.trace_headers(false));
            if let Some((_, password)) = hop.proxy.as_ref().and_then(|p| p.credentials.as_ref()) {
                rec.add_secret(password);
            }
            let resp = self.run(&current, &url, &hop, rec).await?;
            if current.settings.cookies {
                let set_cookie = resp
                    .headers
                    .iter()
                    .filter(|h| h.name.eq_ignore_ascii_case("set-cookie"))
                    .map(|h| h.value.as_str());
                for message in self.inner.cookies.store(&url, set_cookie) {
                    rec.record(TraceEvent::Info { message });
                }
            }
            if !answered && let Some(answer) = crate::auth::digest_retry(&current, &url, &resp) {
                match answer {
                    Ok(value) => {
                        rec.record(TraceEvent::AuthChallenge {
                            status: resp.status,
                            scheme: "digest".into(),
                        });
                        hop = match self.prepare_hop(&current, &url, Some(value)).await {
                            Ok(hop) => hop,
                            Err(e) => {
                                return Err(
                                    rec.fail(format!("cannot answer the challenge: {e}"), false)
                                );
                            }
                        };
                        answered = true;
                        continue;
                    }
                    Err(why) => rec.record(TraceEvent::Info {
                        message: format!("cannot answer the Digest challenge: {why}"),
                    }),
                }
            }
            answered = false;
            if !current.settings.follow_redirects {
                return Ok(resp);
            }
            rec.phase = Phase::Redirect;
            let next = match redirect::next_hop(&current, &url, &resp) {
                Ok(Some(next)) => next,
                Ok(None) => return Ok(resp),
                Err(e) => return Err(rec.fail(e.to_string(), false)),
            };
            if followed >= current.settings.max_redirects {
                return Err(rec.fail(
                    format!(
                        "stopped after {followed} redirect(s); the next one goes to {} (max_redirects = {})",
                        next.url, current.settings.max_redirects
                    ),
                    false,
                ));
            }
            followed += 1;
            rec.record(TraceEvent::Redirect {
                status: resp.status,
                from: url.to_string(),
                to: next.url.to_string(),
            });
            for message in next.notes {
                rec.record(TraceEvent::Info { message });
            }
            let mut next_url = next.url;
            crate::auth::apply_to_url(&next.request.auth, &mut next_url);
            hop = match self.prepare_hop(&next.request, &next_url, None).await {
                Ok(hop) => hop,
                Err(e) => return Err(rec.fail(format!("cannot follow redirect: {e}"), false)),
            };
            current = next.request;
            url = next_url;
        }
    }

    async fn run(
        &self,
        req: &Request,
        url: &url::Url,
        hop: &Hop,
        rec: &mut Recorder,
    ) -> Result<Response> {
        let prepared = &hop.prepared;
        let key = PoolKey::new(url, &req.settings, hop.proxy.as_ref());

        if let Some(conn) = self.reusable_conn(&key).await {
            rec.record(TraceEvent::ConnectionReused { addr: conn.remote });
            match self.exchange(conn, url, prepared, rec, true, &req.settings).await? {
                Outcome::Done(resp, conn, reusable) => {
                    return Ok(self.finish(key, resp, conn, reusable));
                }
                Outcome::Stale(why) => rec.record(TraceEvent::Info {
                    message: format!(
                        "reused connection was closed by the server ({why}); retrying on a new connection"
                    ),
                }),
            }
        }

        let conn = self
            .connect(url, req, hop.tls.as_ref(), hop.proxy.as_ref(), rec)
            .await?;
        // Concurrent requests can use a new HTTP/2 connection immediately.
        self.inner.pool.share(&key, &conn);
        match self
            .exchange(conn, url, prepared, rec, false, &req.settings)
            .await?
        {
            Outcome::Done(resp, conn, reusable) => Ok(self.finish(key, resp, conn, reusable)),
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

    async fn connect(
        &self,
        url: &url::Url,
        req: &Request,
        tls: Option<&RequestTls>,
        proxy: Option<&proxy::Proxy>,
        rec: &mut Recorder,
    ) -> Result<Conn> {
        match req.settings.connect_timeout {
            Some(limit) => {
                match tokio::time::timeout(
                    limit,
                    conn::connect(url, &req.settings, tls, proxy, rec),
                )
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(rec.fail(
                        format!("no connection within {} ms", limit.as_millis()),
                        true,
                    )),
                }
            }
            None => conn::connect(url, &req.settings, tls, proxy, rec).await,
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
        settings: &crate::model::RequestSettings,
    ) -> Result<Outcome> {
        let decompress = settings.decompress;
        let capture = settings
            .capture_wire
            .then_some(self.inner.config.wire_capture_limit);
        let h2 = conn.sender.is_h2();
        let mut trace_headers = prepared.trace_headers(h2);
        if let Some(Some(_)) = &conn.forward_proxy {
            trace_headers.push(Header::new("Proxy-Authorization", "Basic <redacted>"));
        }
        rec.record(TraceEvent::RequestHeaders {
            method: prepared.method().to_string(),
            url: url.to_string(),
            version: if h2 {
                HttpVersion::Http2
            } else {
                HttpVersion::Http11
            },
            headers: trace_headers,
        });
        if capture.is_some() {
            if h2 {
                rec.record(TraceEvent::Info {
                    message: "wire capture is not available for HTTP/2".into(),
                });
            } else {
                rec.watch_capture(conn.meter.clone());
            }
        }
        let request = if let Some(auth) = &conn.forward_proxy {
            conn.meter.reset(capture);
            rec.begin_send(SendTiming::Socket(conn.meter.clone()), prepared.body_len);
            prepared
                .to_http_via_proxy(auth.as_deref())
                .map_err(|e| rec.fail(e.to_string(), false))?
        } else if h2 {
            // The socket is shared by other streams, so time the body instead.
            let body_done = Arc::new(Mutex::new(None));
            rec.begin_send(SendTiming::Stream(body_done.clone()), prepared.body_len);
            prepared.to_http_with_body(true, body::timed(prepared.body_stream(), body_done))
        } else {
            conn.meter.reset(capture);
            rec.begin_send(SendTiming::Socket(conn.meter.clone()), prepared.body_len);
            prepared.to_http(false)
        };

        let response = match conn.sender.send(request).await {
            Ok(r) => r,
            Err(e) => {
                // HTTP/2 "canceled"/"closed" means the stream was never processed.
                let nothing_received = h2 || conn.meter.snapshot().bytes_read == 0;
                if reused
                    && nothing_received
                    && (e.is_canceled() || e.is_closed() || e.is_incomplete_message())
                {
                    rec.finish_send(None);
                    return Ok(Outcome::Stale(error_chain(&e)));
                }
                return Err(rec.fail(error_chain(&e), false));
            }
        };

        let first_byte = rec
            .finish_send(Some(Instant::now()))
            .unwrap_or_else(Instant::now);
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
        rec.emit_capture();

        let body = if decompress {
            let encodings: Vec<&str> = parts
                .headers
                .get_all(header::CONTENT_ENCODING)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect();
            match decode::decode(buf.freeze(), &encodings) {
                decode::DecodeOutcome::Identity(body) => body,
                decode::DecodeOutcome::Decoded { body, encodings } => {
                    rec.record(TraceEvent::Info {
                        message: format!(
                            "decoded {} body: {body_len} → {} bytes",
                            encodings.join(" + "),
                            body.len()
                        ),
                    });
                    body
                }
                decode::DecodeOutcome::Raw { body, reason } => {
                    rec.record(TraceEvent::Info {
                        message: format!("{reason}; showing the raw body"),
                    });
                    body
                }
            }
        } else {
            buf.freeze()
        };

        let decoded_len = body.len() as u64;
        let snap = conn.meter.snapshot();
        let reusable = h2
            || !parts
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
                body,
                remote_addr: Some(conn.remote),
                timings: Timings::default(),
                sizes: Sizes {
                    // HTTP/2 headers are HPACK-compressed on a shared socket: not measurable per request.
                    request_headers: if h2 {
                        0
                    } else {
                        snap.bytes_written.saturating_sub(prepared.body_len)
                    },
                    request_body: prepared.body_len,
                    response_headers: if h2 {
                        0
                    } else {
                        snap.bytes_read.saturating_sub(body_len)
                    },
                    response_body: body_len,
                    response_body_decoded: decoded_len,
                },
                trace: Vec::new(),
            },
            conn,
            reusable,
        ))
    }

    /// Returns the connection to the pool if it can be reused.
    fn finish(&self, key: PoolKey, resp: Response, conn: Conn, reusable: bool) -> Response {
        if reusable {
            self.inner.pool.checkin(key, conn);
        }
        // Dropping a non-reusable `conn` here lets hyper close the connection.
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
