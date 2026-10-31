//! Collects trace events for one execution.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use super::meter::Meter;
use crate::error::{Error, ExecutionError};
use crate::model::{Phase, TraceEntry, TraceEvent};

/// Where the send/wait timings of a request come from.
#[derive(Debug)]
pub(crate) enum SendTiming {
    /// HTTP/1.1: the socket carries one exchange at a time, so its meter is exact.
    Socket(Meter),
    /// HTTP/2: the socket is shared, so use the instant the request body was fully
    /// handed to the stream; the first response byte is approximated by the arrival
    /// of the response headers.
    Stream(Arc<Mutex<Option<Instant>>>),
}

/// A request that is being written.
#[derive(Debug)]
struct PendingSend {
    timing: SendTiming,
    started: Instant,
    body_len: u64,
}

#[derive(Debug)]
pub(crate) struct Recorder {
    start: Instant,
    entries: Vec<TraceEntry>,
    /// The phase currently running; used to label failures and timeouts.
    pub(crate) phase: Phase,
    pending: Option<PendingSend>,
}

impl Recorder {
    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
            entries: Vec::new(),
            phase: Phase::Prepare,
            pending: None,
        }
    }

    /// Records an event that happened now.
    pub(crate) fn record(&mut self, event: TraceEvent) {
        self.record_at(Instant::now(), event);
    }

    /// Records an event that happened at `when` (clamped to the execution start).
    pub(crate) fn record_at(&mut self, when: Instant, event: TraceEvent) {
        let at = when.saturating_duration_since(self.start);
        tracing::trace!(?at, ?event, "trace");
        self.entries.push(TraceEntry { at, event });
    }

    /// Marks the start of writing a request.
    pub(crate) fn begin_send(&mut self, timing: SendTiming, body_len: u64) {
        self.phase = Phase::Send;
        self.pending = Some(PendingSend {
            timing,
            started: Instant::now(),
            body_len,
        });
    }

    /// Emits `RequestSent` and `ResponseFirstByte` as far as they got.
    /// `headers_at` is when the response headers arrived, if they did.
    /// Returns the instant of the first response byte.
    pub(crate) fn finish_send(&mut self, headers_at: Option<Instant>) -> Option<Instant> {
        let p = self.pending.take()?;
        let (done, first) = match &p.timing {
            SendTiming::Socket(meter) => {
                let snap = meter.snapshot();
                (
                    snap.write_done_at.or(snap.last_write_at)?,
                    snap.first_read_at,
                )
            }
            SendTiming::Stream(body_done) => {
                let done = body_done
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    // An empty body is never polled; it is "sent" with the headers.
                    .unwrap_or(p.started);
                (done, headers_at)
            }
        };
        self.record_at(
            done,
            TraceEvent::RequestSent {
                body_bytes: p.body_len,
                duration: done.saturating_duration_since(p.started),
            },
        );
        self.phase = Phase::Wait;
        let first = first?;
        self.record_at(
            first,
            TraceEvent::ResponseFirstByte {
                wait: first.saturating_duration_since(done),
            },
        );
        Some(first)
    }

    /// Moves the recorded entries out.
    pub(crate) fn take_entries(&mut self) -> Vec<TraceEntry> {
        std::mem::take(&mut self.entries)
    }

    /// Records an `Error` event for the current phase and turns the trace into an [`Error`].
    pub(crate) fn fail(&mut self, message: impl Into<String>, timed_out: bool) -> Error {
        self.finish_send(None);
        let message = message.into();
        self.record(TraceEvent::Error {
            phase: self.phase,
            message: message.clone(),
        });
        Error::Execution(Box::new(ExecutionError {
            phase: self.phase,
            timed_out,
            message,
            trace: std::mem::take(&mut self.entries),
        }))
    }
}
