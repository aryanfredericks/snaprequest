//! Errors and warnings from loading and saving collections.

use std::path::{Path, PathBuf};

/// Errors produced by `snapreq-collection`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A file could not be read or written.
    #[error("{}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    /// A file is not valid: bad TOML, an unknown key, a wrong type, a missing value.
    #[error("{}{}: {message}", path.display(), location.map(|(l, c)| format!(":{l}:{c}")).unwrap_or_default())]
    Invalid {
        path: PathBuf,
        /// 1-based line and column, when known.
        location: Option<(usize, usize)>,
        message: String,
    },

    /// The collection was written by a newer SnapRequest.
    #[error("{}: collection format {found} is newer than this SnapRequest understands (up to {supported})", path.display())]
    UnsupportedFormat {
        path: PathBuf,
        found: u32,
        supported: u32,
    },

    /// An error from the core engine.
    #[error(transparent)]
    Core(#[from] snapreq_core::Error),
}

impl Error {
    pub(crate) fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io {
            path: path.to_owned(),
            source,
        }
    }

    pub(crate) fn invalid(path: &Path, message: impl Into<String>) -> Self {
        Self::Invalid {
            path: path.to_owned(),
            location: None,
            message: message.into(),
        }
    }

    /// Turns a TOML parse error into an `Invalid` error with line and column.
    pub(crate) fn toml(path: &Path, text: &str, err: &toml::de::Error) -> Self {
        let location = err.span().map(|span| line_col(text, span.start));
        Self::Invalid {
            path: path.to_owned(),
            location,
            message: err.message().trim().to_owned(),
        }
    }
}

/// 1-based line and column of a byte offset.
fn line_col(text: &str, offset: usize) -> (usize, usize) {
    let before = &text[..offset.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, column)
}

/// Something worth telling the user that does not stop the collection from loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Warning {
    pub path: PathBuf,
    pub message: String,
}

impl std::fmt::Display for Warning {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path.display(), self.message)
    }
}

/// Convenience alias for results in `snapreq-collection`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_and_column() {
        let text = "a = 1\nbb = 2\n";
        assert_eq!(line_col(text, 0), (1, 1));
        assert_eq!(line_col(text, 6), (2, 1));
        assert_eq!(line_col(text, 9), (2, 4));
    }

    #[test]
    fn invalid_display() {
        let e = Error::Invalid {
            path: "c/a.toml".into(),
            location: Some((3, 5)),
            message: "unknown field `methd`".into(),
        };
        assert_eq!(e.to_string(), "c/a.toml:3:5: unknown field `methd`");
    }
}
