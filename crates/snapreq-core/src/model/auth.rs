//! Authentication attached to a request.

use serde::{Deserialize, Serialize};

/// How a request authenticates. The engine applies it to every hop it sends, and drops it
/// when a redirect leaves the original origin (like an `Authorization` header).
///
/// Every credential value is hidden in traces.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Auth {
    /// No authentication helper; headers set by hand still go out.
    #[default]
    None,
    /// `Authorization: Basic base64(username:password)` (RFC 7617, UTF-8).
    Basic {
        username: String,
        #[serde(default)]
        password: String,
    },
    /// `Authorization: Bearer <token>` (RFC 6750).
    Bearer { token: String },
    /// HTTP Digest (RFC 7616). The request is sent without credentials first; on a
    /// `401` with a Digest challenge it is sent again once, with the answer.
    Digest {
        username: String,
        #[serde(default)]
        password: String,
    },
    /// A key sent in a header or a query parameter.
    ApiKey {
        /// Header or query parameter name, for example `X-API-Key` or `api_key`.
        name: String,
        value: String,
        #[serde(default)]
        location: ApiKeyLocation,
    },
}

/// Where an API key is sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyLocation {
    #[default]
    Header,
    Query,
}

impl Auth {
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// Short name for messages: `basic`, `bearer`, `api_key`, `none`.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Basic { .. } => "basic",
            Self::Bearer { .. } => "bearer",
            Self::Digest { .. } => "digest",
            Self::ApiKey { .. } => "api_key",
        }
    }

    /// Values that must never appear in traces or output.
    pub fn secrets(&self) -> Vec<String> {
        let mut out = match self {
            Self::None => Vec::new(),
            Self::Basic { username, password } => {
                vec![password.clone(), basic_credentials(username, password)]
            }
            Self::Bearer { token } => vec![token.clone()],
            Self::Digest { password, .. } => vec![password.clone()],
            Self::ApiKey { value, .. } => vec![value.clone()],
        };
        out.retain(|s| !s.is_empty());
        out
    }
}

/// `base64(username:password)`, the credential of a Basic `Authorization` header.
pub(crate) fn basic_credentials(username: &str, password: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(format!("{username}:{password}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_shape() {
        let a: Auth = serde_json::from_str(
            r#"{"type":"api_key","name":"api_key","value":"k","location":"query"}"#,
        )
        .unwrap();
        assert_eq!(
            a,
            Auth::ApiKey {
                name: "api_key".into(),
                value: "k".into(),
                location: ApiKeyLocation::Query
            }
        );
        let b: Auth = serde_json::from_str(r#"{"type":"basic","username":"u"}"#).unwrap();
        assert_eq!(
            serde_json::to_string(&b).unwrap(),
            r#"{"type":"basic","username":"u","password":""}"#
        );
        assert!(serde_json::from_str::<Auth>(r#"{"type":"bearer","tokn":"x"}"#).is_err());
    }

    #[test]
    fn secrets_include_encoded_basic_credentials() {
        let a = Auth::Basic {
            username: "Aladdin".into(),
            password: "open sesame".into(),
        };
        assert_eq!(a.secrets(), ["open sesame", "QWxhZGRpbjpvcGVuIHNlc2FtZQ=="]);
        assert!(Auth::None.secrets().is_empty());
    }
}
