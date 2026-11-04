//! Import errors.

/// Errors from importing.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The input's format could not be recognised.
    #[error("unrecognized input: {0}")]
    Unrecognized(String),
    /// The input is in a known format but cannot be read.
    #[error("invalid {format} input: {message}")]
    Invalid {
        format: &'static str,
        message: String,
    },
    /// The input uses something SnapRequest cannot import.
    #[error("{0}")]
    Unsupported(String),
    /// The target directory already holds files.
    #[error("`{0}` already exists and is not empty")]
    TargetExists(std::path::PathBuf),
    /// Writing the collection failed.
    #[error(transparent)]
    Collection(#[from] snapreq_collection::Error),
}

impl Error {
    pub(crate) fn invalid(format: &'static str, message: impl Into<String>) -> Self {
        Self::Invalid {
            format,
            message: message.into(),
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
