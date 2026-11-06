//! Server-Sent Events (PLAN P10.2): the `text/event-stream` format of the HTML standard, parsed
//! incrementally as bytes arrive.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::client::BodyObserver;
use crate::model::Header;

/// One dispatched event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SseEvent {
    /// The `event` field; `message` when the server sent none.
    pub event: String,
    /// The `data` lines joined with `\n`.
    pub data: String,
    /// The last event ID seen (it carries over to later events, as in browsers).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// A `retry` field sent with this event (milliseconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<u64>,
}

/// Whether a `Content-Type` value is `text/event-stream`.
pub fn is_event_stream(content_type: &str) -> bool {
    content_type
        .split(';')
        .next()
        .is_some_and(|t| t.trim().eq_ignore_ascii_case("text/event-stream"))
}

/// Parses an event stream fed in arbitrary pieces.
#[derive(Debug, Default)]
pub struct SseParser {
    /// Bytes of an unfinished line.
    line: Vec<u8>,
    /// The previous chunk ended with `\r` (a following `\n` belongs to it).
    after_cr: bool,
    started: bool,
    event: String,
    data: String,
    has_data: bool,
    retry: Option<u64>,
    last_id: Option<String>,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds bytes; returns the events completed by them.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<SseEvent> {
        let mut out = Vec::new();
        for &b in bytes {
            if self.after_cr {
                self.after_cr = false;
                if b == b'\n' {
                    continue;
                }
            }
            match b {
                b'\r' => {
                    self.after_cr = true;
                    self.end_line(&mut out);
                }
                b'\n' => self.end_line(&mut out),
                _ => self.line.push(b),
            }
        }
        out
    }

    /// Events of a complete stream (an unterminated last event is dropped, as in browsers).
    pub fn parse_all(bytes: &[u8]) -> Vec<SseEvent> {
        Self::new().feed(bytes)
    }

    fn end_line(&mut self, out: &mut Vec<SseEvent>) {
        let mut line = std::mem::take(&mut self.line);
        if !self.started {
            self.started = true;
            if line.starts_with(b"\xEF\xBB\xBF") {
                line.drain(..3);
            }
        }
        if line.is_empty() {
            if self.has_data {
                let mut data = std::mem::take(&mut self.data);
                if data.ends_with('\n') {
                    data.pop();
                }
                out.push(SseEvent {
                    event: if self.event.is_empty() {
                        "message".into()
                    } else {
                        std::mem::take(&mut self.event)
                    },
                    data,
                    id: self.last_id.clone(),
                    retry: self.retry.take(),
                });
            }
            self.event.clear();
            self.data.clear();
            self.has_data = false;
            return;
        }
        let text = String::from_utf8_lossy(&line);
        if text.starts_with(':') {
            return; // a comment
        }
        let (field, value) = match text.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (text.as_ref(), ""),
        };
        match field {
            "event" => self.event = value.to_owned(),
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
                self.has_data = true;
            }
            "id" if !value.contains('\0') => self.last_id = Some(value.to_owned()),
            "retry" => {
                if let Ok(ms) = value.parse() {
                    self.retry = Some(ms);
                }
            }
            _ => {}
        }
    }
}

/// An event with the time it arrived, counted from the start of the response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedEvent {
    #[serde(flatten)]
    pub event: SseEvent,
    #[serde(with = "crate::serde_util::duration_ms", rename = "at_ms")]
    pub at: std::time::Duration,
}

/// A [`BodyObserver`] that parses `text/event-stream` responses and calls `on_event` for each
/// event as it arrives. Other responses are ignored; every new response (after a redirect or
/// an auth retry) starts a fresh parser.
pub fn observer(on_event: impl Fn(&TimedEvent) + Send + Sync + 'static) -> Arc<dyn BodyObserver> {
    Arc::new(SseObserver {
        state: Mutex::new(None),
        on_event: Box::new(on_event),
    })
}

type OnEvent = Box<dyn Fn(&TimedEvent) + Send + Sync>;

struct SseObserver {
    /// The parser and start time while an event stream is being read.
    state: Mutex<Option<(SseParser, Instant)>>,
    on_event: OnEvent,
}

impl std::fmt::Debug for SseObserver {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SseObserver")
    }
}

impl BodyObserver for SseObserver {
    fn head(&self, _status: u16, headers: &[Header]) {
        let sse = headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("content-type") && is_event_stream(&h.value));
        *self.state.lock().unwrap_or_else(|e| e.into_inner()) =
            sse.then(|| (SseParser::new(), Instant::now()));
    }

    fn chunk(&self, data: &[u8]) {
        let events = {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match state.as_mut() {
                Some((parser, start)) => {
                    let at = start.elapsed();
                    parser
                        .feed(data)
                        .into_iter()
                        .map(|event| TimedEvent { event, at })
                        .collect()
                }
                None => Vec::new(),
            }
        };
        for e in &events {
            (self.on_event)(e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(event: &str, data: &str, id: Option<&str>) -> SseEvent {
        SseEvent {
            event: event.into(),
            data: data.into(),
            id: id.map(str::to_owned),
            retry: None,
        }
    }

    #[test]
    fn parses_the_standard_examples() {
        let events = SseParser::parse_all(
            b": a comment\n\ndata: first\ndata: second\n\nid: 7\nevent: price\ndata:{\"p\":1}\n\ndata\n\ndata:  two spaces\n\n",
        );
        assert_eq!(
            events,
            [
                ev("message", "first\nsecond", None),
                ev("price", "{\"p\":1}", Some("7")),
                ev("message", "", Some("7")),
                ev("message", " two spaces", Some("7")),
            ]
        );
    }

    #[test]
    fn chunks_can_split_anywhere_and_any_line_ending_works() {
        let text = b"\xEF\xBB\xBFretry: 3000\r\ndata: a\r\n\r\ndata: b\r\rdata: c\n\n";
        let mut all = Vec::new();
        let mut p = SseParser::new();
        for b in text {
            all.extend(p.feed(&[*b]));
        }
        assert_eq!(all.len(), 3);
        assert_eq!(all[0].data, "a");
        assert_eq!(all[0].retry, Some(3000));
        assert_eq!(all[1].data, "b");
        assert_eq!(all[1].retry, None);
        assert_eq!(all[2].data, "c");
        // An unterminated event is not dispatched.
        assert!(SseParser::parse_all(b"data: x").is_empty());
        // Events without data are not dispatched; the type resets.
        assert_eq!(
            SseParser::parse_all(b"event: x\n\ndata: y\n\n"),
            [ev("message", "y", None)]
        );
    }

    #[test]
    fn content_types() {
        assert!(is_event_stream("text/event-stream; charset=utf-8"));
        assert!(is_event_stream("Text/Event-Stream"));
        assert!(!is_event_stream("text/plain"));
    }
}
