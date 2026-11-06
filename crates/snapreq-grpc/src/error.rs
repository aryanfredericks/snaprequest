//! Errors of `snapreq-grpc`.

/// What can go wrong before a call has a gRPC status.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The request (URL, auth, settings) or the call is invalid.
    #[error("{0}")]
    Invalid(String),
    /// Proto files do not compile.
    #[error("proto files: {0}")]
    Proto(String),
    /// Server reflection failed.
    #[error("server reflection: {0}")]
    Reflection(String),
    /// Connecting failed (with the trace).
    #[error(transparent)]
    Core(#[from] snapreq_core::Error),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
