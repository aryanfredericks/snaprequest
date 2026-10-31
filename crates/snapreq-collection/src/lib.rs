//! SnapRequest collection and environment file format (TOML), variables and interpolation.

/// Errors produced by `snapreq-collection`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An error from the core HTTP engine.
    #[error(transparent)]
    Core(#[from] snapreq_core::Error),
}

/// Convenience alias for results in `snapreq-collection`.
pub type Result<T, E = Error> = std::result::Result<T, E>;
