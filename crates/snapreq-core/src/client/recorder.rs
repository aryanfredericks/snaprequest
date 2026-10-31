//! Collects trace events for one execution.

use std::time::Instant;

use super::meter::Meter;
use crate::error::{Error, ExecutionError};
use crate::model::{Phase, TraceEntry, TraceEvent};

/// A request that is being written; its timings are read from the socket meter.
#[derive(Debug)]
struct PendingSend {
    meter: Meter,
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

    /// Marks the start of writing a request on a connection measured by `meter`.
    pub(crate) fn begin_send(&mut self, meter: Meter, body_len: u64) {
        self.phase = Phase::Send;
        self.pending = Some(PendingSend {
            meter,
            started: Instant::now(),
            body_len,
        });
    }

    /// Emits `RequestSent` and `ResponseFirstByte` from the socket measurements,
    /// as far as they got. Returns the instant of the first response byte.
    pub(crate) fn finish_send(&mut self) -> Option<Instant> {
        let p = self.pending.take()?;
        let snap = p.meter.snapshot();
        let done = snap.write_done_at.or(snap.last_write_at)?;
        self.record_at(
            done,
            TraceEvent::RequestSent {
                body_bytes: p.body_len,
                duration: done.saturating_duration_since(p.started),
            },
        );
        self.phase = Phase::Wait;
        let first = snap.first_read_at?;
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
        self.finish_send();
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
