//! WebSocket client (PLAN P10.1, ADR-033).
//!
//! The opening handshake is an ordinary HTTP/1.1 request sent by the engine, so DNS, the
//! outbound policy, proxies, TLS settings, auth, cookies and the trace work as for any request.
//! After `101 Switching Protocols` the connection is handed to `tungstenite` for framing.

use std::time::{Duration, Instant};

use base64::Engine as _;
use bytes::Bytes;
use futures_util::stream::{SplitSink, SplitStream};
use futures_util::{SinkExt, StreamExt};
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use tokio_tungstenite::WebSocketStream;
use tokio_tungstenite::tungstenite::protocol::{CloseFrame, Role, WebSocketConfig};
use tokio_tungstenite::tungstenite::{self, Message};

use super::conn::Sender;
use super::recorder::{Recorder, SendTiming};
use super::{Client, TraceSink, error_chain, recorder};
use crate::model::{
    Auth, Header, HttpVersion, HttpVersionPref, Phase, Request, Response, Sizes, Timings,
    TraceEvent,
};
use crate::{Error, Result};

/// The GUID of RFC 6455, section 1.3.
const ACCEPT_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

/// Largest message accepted from the server.
pub const MAX_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

/// Options of a WebSocket connection beyond the request itself.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WsOptions {
    /// Subprotocols offered in `Sec-WebSocket-Protocol`, in order of preference.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subprotocols: Vec<String>,
}

/// One WebSocket message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WsMessage {
    Text {
        data: String,
    },
    /// Binary data, base64 in JSON.
    Binary {
        #[serde(with = "b64")]
        data: Bytes,
    },
    Ping {
        #[serde(with = "b64", default)]
        data: Bytes,
    },
    Pong {
        #[serde(with = "b64", default)]
        data: Bytes,
    },
    Close {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<u16>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        reason: String,
    },
}

impl WsMessage {
    /// Payload size in bytes.
    pub fn len(&self) -> usize {
        match self {
            Self::Text { data } => data.len(),
            Self::Binary { data } | Self::Ping { data } | Self::Pong { data } => data.len(),
            Self::Close { reason, .. } => reason.len() + 2,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn from_tungstenite(m: Message) -> Option<Self> {
        Some(match m {
            Message::Text(t) => Self::Text {
                data: t.as_str().to_owned(),
            },
            Message::Binary(b) => Self::Binary { data: b },
            Message::Ping(b) => Self::Ping { data: b },
            Message::Pong(b) => Self::Pong { data: b },
            Message::Close(frame) => match frame {
                Some(f) => Self::Close {
                    code: Some(f.code.into()),
                    reason: f.reason.as_str().to_owned(),
                },
                None => Self::Close {
                    code: None,
                    reason: String::new(),
                },
            },
            Message::Frame(_) => return None,
        })
    }

    fn into_tungstenite(self) -> Message {
        match self {
            Self::Text { data } => Message::text(data),
            Self::Binary { data } => Message::Binary(data),
            Self::Ping { data } => Message::Ping(data),
            Self::Pong { data } => Message::Pong(data),
            Self::Close { code, reason } => Message::Close(code.map(|c| CloseFrame {
                code: c.into(),
                reason: reason.into(),
            })),
        }
    }
}

mod b64 {
    use base64::Engine as _;
    use bytes::Bytes;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &Bytes, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&base64::engine::general_purpose::STANDARD.encode(b))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Bytes, D::Error> {
        let s = String::deserialize(d)?;
        base64::engine::general_purpose::STANDARD
            .decode(s.trim())
            .map(Bytes::from)
            .map_err(|e| serde::de::Error::custom(format!("invalid base64: {e}")))
    }
}

type Stream = WebSocketStream<TokioIo<hyper::upgrade::Upgraded>>;

/// An open WebSocket connection.
#[derive(Debug)]
pub struct WebSocket {
    /// The `101` handshake response, with the trace of the connection.
    pub handshake: Response,
    /// The subprotocol the server chose, if any.
    pub protocol: Option<String>,
    stream: Stream,
}

/// The sending half of a [`WebSocket`].
#[derive(Debug)]
pub struct WsSender(SplitSink<Stream, Message>);

/// The receiving half of a [`WebSocket`].
#[derive(Debug)]
pub struct WsReceiver(SplitStream<Stream>);

fn ws_error(e: &tungstenite::Error) -> Error {
    Error::WebSocket(error_chain(e))
}

impl WebSocket {
    /// Splits the connection so messages can be sent while others are received.
    pub fn split(self) -> (WsSender, WsReceiver) {
        let (tx, rx) = self.stream.split();
        (WsSender(tx), WsReceiver(rx))
    }
}

impl WsSender {
    /// Sends one message. Sending `Close` starts the closing handshake.
    pub async fn send(&mut self, msg: WsMessage) -> Result<()> {
        self.0
            .send(msg.into_tungstenite())
            .await
            .map_err(|e| ws_error(&e))
    }

    /// Sends a close frame (normal closure unless `code` is given).
    pub async fn close(&mut self, code: Option<u16>, reason: &str) -> Result<()> {
        let msg = WsMessage::Close {
            code: Some(code.unwrap_or(1000)),
            reason: reason.to_owned(),
        };
        match self.0.send(msg.into_tungstenite()).await {
            Ok(())
            | Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                Ok(())
            }
            Err(e) => Err(ws_error(&e)),
        }
    }
}

impl WsReceiver {
    /// The next message; `None` once the connection is closed. Pings are answered
    /// automatically (and still returned, for the log).
    pub async fn recv(&mut self) -> Option<Result<WsMessage>> {
        loop {
            match self.0.next().await? {
                Ok(m) => {
                    if let Some(m) = WsMessage::from_tungstenite(m) {
                        return Some(Ok(m));
                    }
                }
                // Many servers drop the connection without a close frame; that is the end too.
                Err(
                    tungstenite::Error::ConnectionClosed
                    | tungstenite::Error::AlreadyClosed
                    | tungstenite::Error::Protocol(
                        tungstenite::error::ProtocolError::ResetWithoutClosingHandshake,
                    ),
                ) => {
                    return None;
                }
                Err(e) => return Some(Err(ws_error(&e))),
            }
        }
    }
}

/// `ws://` → `http://`, `wss://` → `https://` (and back for display).
pub(crate) fn http_url_of(url: &url::Url) -> Result<url::Url> {
    let scheme = match url.scheme() {
        "ws" | "http" => "http",
        "wss" | "https" => "https",
        other => {
            return Err(Error::InvalidUrl {
                url: url.to_string(),
                reason: format!("unsupported scheme `{other}` (expected ws or wss)"),
            });
        }
    };
    let rest = &url.as_str()[url.scheme().len()..];
    url::Url::parse(&format!("{scheme}{rest}")).map_err(|e| Error::InvalidUrl {
        url: url.to_string(),
        reason: e.to_string(),
    })
}

fn accept_key(key: &str) -> String {
    let digest = ring::digest::digest(
        &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        format!("{key}{ACCEPT_GUID}").as_bytes(),
    );
    base64::engine::general_purpose::STANDARD.encode(digest.as_ref())
}

fn has_header(req: &Request, name: &str) -> bool {
    req.enabled_headers()
        .any(|h| h.name.trim().eq_ignore_ascii_case(name))
}

impl Client {
    /// Opens a WebSocket connection: `req.url` is a `ws://` or `wss://` URL (`http(s)://` is
    /// accepted too); headers, query, auth and settings apply to the opening handshake.
    ///
    /// Redirects are not followed. When the server does not switch protocols, the error
    /// names the status and the start of its answer.
    pub async fn websocket(
        &self,
        req: &Request,
        options: &WsOptions,
        sink: Option<TraceSink>,
    ) -> Result<WebSocket> {
        let mut req = req.clone();
        let ws_url = req.effective_ws_url()?;
        let url = http_url_of(&ws_url)?;
        // The upgrade needs HTTP/1.1 (RFC 8441 over HTTP/2 is rarely supported).
        req.settings.http_version = HttpVersionPref::Http1;
        req.settings.follow_redirects = false;
        req.body = crate::Body::None;
        let mut key = [0u8; 16];
        ring::rand::SecureRandom::fill(&ring::rand::SystemRandom::new(), &mut key)
            .map_err(|_| Error::InvalidRequest("no random source for the WebSocket key".into()))?;
        let key = base64::engine::general_purpose::STANDARD.encode(key);
        for (name, value) in [
            ("Connection", "Upgrade".to_owned()),
            ("Upgrade", "websocket".to_owned()),
            ("Sec-WebSocket-Version", "13".to_owned()),
            ("Sec-WebSocket-Key", key.clone()),
        ] {
            if !has_header(&req, name) {
                req.headers.push(crate::model::KeyValue::new(name, value));
            }
        }
        if !options.subprotocols.is_empty() && !has_header(&req, "sec-websocket-protocol") {
            req.headers.push(crate::model::KeyValue::new(
                "Sec-WebSocket-Protocol",
                options.subprotocols.join(", "),
            ));
        }
        let mut rec = self.recorder(&req);
        rec.sink = sink.map(recorder::Sink);
        let token = match &req.auth {
            Auth::OAuth2(o) if crate::auth::shadowed_by_header(&req).is_none() => {
                Some(self.oauth_token(&req, o, &mut rec, false).await?.header())
            }
            _ => None,
        };
        let work = self.handshake(&req, &url, token, &mut rec);
        let result = match req.settings.timeout {
            Some(limit) => match tokio::time::timeout(limit, work).await {
                Ok(r) => r,
                Err(_) => Err(rec.fail(
                    format!(
                        "the WebSocket handshake did not complete within {} ms",
                        limit.as_millis()
                    ),
                    true,
                )),
            },
            None => work.await,
        };
        let (mut handshake, upgraded) = result?;
        let accept = handshake
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("sec-websocket-accept"))
            .map(|h| h.value.trim().to_owned());
        if accept.as_deref() != Some(accept_key(&key).as_str()) {
            return Err(rec.fail(
                "the server's Sec-WebSocket-Accept does not match the key that was sent",
                false,
            ));
        }
        let protocol = handshake
            .headers
            .iter()
            .find(|h| h.name.eq_ignore_ascii_case("sec-websocket-protocol"))
            .map(|h| h.value.trim().to_owned());
        rec.record(TraceEvent::Info {
            message: match &protocol {
                Some(p) => format!("WebSocket open (subprotocol {p})"),
                None => "WebSocket open".into(),
            },
        });
        handshake.url = ws_url.to_string();
        handshake.trace = rec.take_entries();
        handshake.timings = Timings::from_trace(&handshake.trace);
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_SIZE))
            .max_frame_size(Some(MAX_MESSAGE_SIZE));
        let stream =
            WebSocketStream::from_raw_socket(TokioIo::new(upgraded), Role::Client, Some(config))
                .await;
        Ok(WebSocket {
            handshake,
            protocol,
            stream,
        })
    }

    async fn handshake(
        &self,
        req: &Request,
        url: &url::Url,
        token: Option<String>,
        rec: &mut Recorder,
    ) -> Result<(Response, hyper::upgrade::Upgraded)> {
        let hop = self.prepare_hop(req, url, token).await?;
        rec.learn_secrets_from_headers(&hop.prepared.trace_headers(false));
        if let Some((_, password)) = hop.proxy.as_ref().and_then(|p| p.credentials.as_ref()) {
            rec.add_secret(password);
        }
        let mut conn = self
            .connect(url, req, hop.tls.as_ref(), hop.proxy.as_ref(), rec)
            .await?;
        let mut headers = hop.prepared.trace_headers(false);
        if let Some(Some(_)) = &conn.forward_proxy {
            headers.push(Header::new("Proxy-Authorization", "Basic <redacted>"));
        }
        rec.record(TraceEvent::RequestHeaders {
            method: "GET".into(),
            url: url.to_string(),
            version: HttpVersion::Http11,
            headers,
        });
        conn.meter.reset(None);
        rec.begin_send(SendTiming::Socket(conn.meter.clone()), 0);
        let request = match &conn.forward_proxy {
            Some(auth) => hop
                .prepared
                .to_http_via_proxy(auth.as_deref())
                .map_err(|e| rec.fail(e.to_string(), false))?,
            None => hop.prepared.to_http(false),
        };
        let Sender::H1(_) = &conn.sender else {
            return Err(rec.fail("the connection is not HTTP/1.1", false));
        };
        let response = conn
            .sender
            .send(request)
            .await
            .map_err(|e| rec.fail(error_chain(&e), false))?;
        rec.finish_send(Some(Instant::now()));
        let status = response.status().as_u16();
        let headers: Vec<Header> = response
            .headers()
            .iter()
            .map(|(n, v)| Header::new(n.as_str(), String::from_utf8_lossy(v.as_bytes())))
            .collect();
        rec.record(TraceEvent::ResponseHeaders {
            status,
            version: HttpVersion::Http11,
            headers: headers.clone(),
        });
        if status != 101 {
            rec.phase = Phase::Download;
            let body = read_some(response.into_body(), 1024).await;
            let text = String::from_utf8_lossy(&body);
            let text = text.trim();
            return Err(rec.fail(
                if text.is_empty() {
                    format!("the server answered {status} instead of 101 Switching Protocols")
                } else {
                    format!(
                        "the server answered {status} instead of 101 Switching Protocols: {text}"
                    )
                },
                false,
            ));
        }
        let snap = conn.meter.snapshot();
        let resp = Response {
            status,
            version: HttpVersion::Http11,
            url: url.to_string(),
            headers,
            body: Bytes::new(),
            remote_addr: Some(conn.remote),
            timings: Timings::default(),
            sizes: Sizes {
                request_headers: snap.bytes_written,
                request_body: 0,
                response_headers: snap.bytes_read,
                response_body: 0,
                response_body_decoded: 0,
            },
            trace: Vec::new(),
        };
        rec.record(TraceEvent::ResponseComplete {
            body_bytes: 0,
            download: Duration::ZERO,
        });
        let upgraded = hyper::upgrade::on(response)
            .await
            .map_err(|e| rec.fail(format!("upgrade failed: {}", error_chain(&e)), false))?;
        // Keep the connection's task alive: dropping `conn` only drops our handle.
        drop(conn);
        Ok((resp, upgraded))
    }
}

/// Reads up to `limit` bytes of a body (for error messages).
async fn read_some(mut body: hyper::body::Incoming, limit: usize) -> Vec<u8> {
    use http_body_util::BodyExt;
    let mut out = Vec::new();
    while out.len() < limit {
        match tokio::time::timeout(Duration::from_secs(2), body.frame()).await {
            Ok(Some(Ok(frame))) => {
                if let Ok(data) = frame.into_data() {
                    out.extend_from_slice(&data);
                }
            }
            _ => break,
        }
    }
    out.truncate(limit);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accept_key_matches_rfc_example() {
        assert_eq!(
            accept_key("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
    }

    #[test]
    fn urls_map_to_http() {
        let u = url::Url::parse("wss://a.test:9000/chat?x=1").unwrap();
        assert_eq!(
            http_url_of(&u).unwrap().as_str(),
            "https://a.test:9000/chat?x=1"
        );
        let u = url::Url::parse("ws://a.test/").unwrap();
        assert_eq!(http_url_of(&u).unwrap().as_str(), "http://a.test/");
        let u = url::Url::parse("ftp://a.test/").unwrap();
        assert!(http_url_of(&u).is_err());
    }

    #[test]
    fn messages_serialize_with_a_kind() {
        let m = WsMessage::Binary {
            data: Bytes::from_static(b"\x00\x01"),
        };
        let json = serde_json::to_string(&m).unwrap();
        assert_eq!(json, r#"{"kind":"binary","data":"AAE="}"#);
        assert_eq!(serde_json::from_str::<WsMessage>(&json).unwrap(), m);
        let close: WsMessage = serde_json::from_str(r#"{"kind":"close"}"#).unwrap();
        assert_eq!(
            close,
            WsMessage::Close {
                code: None,
                reason: String::new()
            }
        );
    }
}
