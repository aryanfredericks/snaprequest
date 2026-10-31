//! Error type for `snapreq-core`.

use crate::model::{Phase, TraceEntry};

/// Errors produced by the core HTTP engine.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The request URL could not be parsed or is not supported.
    #[error("invalid URL `{url}`: {reason}")]
    InvalidUrl { url: String, reason: String },

    /// The request is malformed (bad header name or value, and so on).
    #[error("invalid request: {0}")]
    InvalidRequest(String),

    /// The request uses a feature this version of SnapRequest cannot do yet.
    #[error("not supported yet: {0}")]
    Unsupported(String),

    /// The server used an HTTP version SnapRequest does not know.
    #[error("unsupported HTTP version {0}")]
    UnsupportedVersion(String),

    /// Sending the request failed. Carries the trace recorded up to the failure.
    #[error(transparent)]
    Execution(Box<ExecutionError>),
}

/// A failure while executing a request, with everything recorded until then.
#[derive(Debug, thiserror::Error)]
#[error("{phase} failed{}: {message}", if *.timed_out { " (timed out)" } else { "" })]
pub struct ExecutionError {
    /// Phase in which execution stopped.
    pub phase: Phase,
    /// `true` if a connect or total timeout fired.
    pub timed_out: bool,
    /// Human-readable cause.
    pub message: String,
    /// Trace up to and including the final `Error` event.
    pub trace: Vec<TraceEntry>,
}

impl Error {
    /// The partial trace, if this error happened during execution.
    pub fn trace(&self) -> Option<&[TraceEntry]> {
        match self {
            Self::Execution(e) => Some(&e.trace),
            _ => None,
        }
    }
}

/// Convenience alias for results in `snapreq-core`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn invalid_url_message() {
        let err = Error::InvalidUrl {
            url: "ht!tp://x".into(),
            reason: "bad scheme".into(),
        };
        assert_eq!(err.to_string(), "invalid URL `ht!tp://x`: bad scheme");
    }

    #[test]
    fn execution_error_message() {
        let err = Error::Execution(Box::new(ExecutionError {
            phase: Phase::Wait,
            timed_out: true,
            message: "no response after 5000 ms".into(),
            trace: vec![],
        }));
        assert_eq!(
            err.to_string(),
            "waiting for response failed (timed out): no response after 5000 ms"
        );
        assert_eq!(err.trace(), Some(&[][..]));
    }
}
