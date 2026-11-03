//! Authentication attached to a request.

use serde::{Deserialize, Serialize};

use super::KeyValue;

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
    /// OAuth 2.0 (RFC 6749): an access token is obtained from `token_url` (and cached by
    /// the client), then sent as `Authorization: Bearer <token>`.
    #[serde(rename = "oauth2")]
    OAuth2(OAuth2),
    /// A key sent in a header or a query parameter.
    ApiKey {
        /// Header or query parameter name, for example `X-API-Key` or `api_key`.
        name: String,
        value: String,
        #[serde(default)]
        location: ApiKeyLocation,
    },
}

/// OAuth 2.0 settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuth2 {
    pub grant: OAuth2Grant,
    /// Token endpoint.
    pub token_url: String,
    /// Authorization endpoint; required for the authorization code grant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    pub client_id: String,
    /// Empty for public clients.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_secret: String,
    /// Space-separated scopes; empty means none requested.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    /// Resource owner credentials for the password grant.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    /// Redirect URI for the authorization code grant. Must be a loopback `http` URL
    /// (`http://127.0.0.1:8765/callback`); SnapRequest listens there for the browser.
    /// Default: `http://127.0.0.1:<random port>/callback`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    /// How the client authenticates at the token endpoint.
    #[serde(default)]
    pub client_auth: ClientAuth,
    /// Extra parameters for the authorization and token requests (for example `audience`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<KeyValue>,
}

impl OAuth2 {
    pub fn new(
        grant: OAuth2Grant,
        token_url: impl Into<String>,
        client_id: impl Into<String>,
    ) -> Self {
        Self {
            grant,
            token_url: token_url.into(),
            auth_url: None,
            client_id: client_id.into(),
            client_secret: String::new(),
            scope: String::new(),
            username: String::new(),
            password: String::new(),
            redirect_uri: None,
            client_auth: ClientAuth::default(),
            params: Vec::new(),
        }
    }
}

/// OAuth 2.0 grant types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuth2Grant {
    ClientCredentials,
    /// Resource owner password credentials (legacy, but common in internal APIs).
    Password,
    /// Authorization code with PKCE (S256), answered in the browser.
    AuthorizationCode,
}

impl OAuth2Grant {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ClientCredentials => "client_credentials",
            Self::Password => "password",
            Self::AuthorizationCode => "authorization_code",
        }
    }
}

/// How the client id and secret are sent to the token endpoint.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ClientAuth {
    /// HTTP Basic (`client_secret_basic`, RFC 6749 section 2.3.1).
    #[default]
    Basic,
    /// In the form body (`client_secret_post`).
    Body,
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
            Self::OAuth2(_) => "oauth2",
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
            Self::OAuth2(o) => vec![o.client_secret.clone(), o.password.clone()],
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

        let o: Auth = serde_json::from_str(
            r#"{"type":"oauth2","grant":"client_credentials","token_url":"https://t/token","client_id":"c"}"#,
        )
        .unwrap();
        assert_eq!(
            o,
            Auth::OAuth2(OAuth2::new(
                OAuth2Grant::ClientCredentials,
                "https://t/token",
                "c"
            ))
        );
        assert_eq!(
            serde_json::to_string(&o).unwrap(),
            r#"{"type":"oauth2","grant":"client_credentials","token_url":"https://t/token","client_id":"c","client_auth":"basic"}"#
        );
        assert!(
            serde_json::from_str::<Auth>(r#"{"type":"oauth2","grant":"password","token_url":"t","client_id":"c","scopes":"x"}"#)
                .is_err()
        );
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
