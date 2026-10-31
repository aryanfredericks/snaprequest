//! Error type for `snapreq-core`.

/// Errors produced by the core HTTP engine.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The request URL could not be parsed or is not supported.
    #[error("invalid URL `{url}`: {reason}")]
    InvalidUrl { url: String, reason: String },

    /// The server used an HTTP version SnapRequest does not know.
    #[error("unsupported HTTP version {0}")]
    UnsupportedVersion(String),
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
}
