//! SnapRequest self-hosted server: REST and WebSocket API, storage and authentication.

/// Errors produced by `snapreq-server`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An error from the core HTTP engine.
    #[error(transparent)]
    Core(#[from] snapreq_core::Error),
}

/// Convenience alias for results in `snapreq-server`.
pub type Result<T, E = Error> = std::result::Result<T, E>;
