//! SnapRequest assertions, collection runner and reporters.

pub mod assert;

pub use assert::{Assertions, Outcome};

/// Errors produced by `snapreq-test`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An error from the core HTTP engine.
    #[error(transparent)]
    Core(#[from] snapreq_core::Error),
}

/// Convenience alias for results in `snapreq-test`.
pub type Result<T, E = Error> = std::result::Result<T, E>;
