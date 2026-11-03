//! The TOML file types of collection format v1 (see `docs/FORMAT.md`).

use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize};
use snapreq_core::model::{
    ApiKeyLocation, ClientAuth, HttpVersionPref, OAuth2Grant, ProxyMode, TlsVersion,
};

use crate::assert::AssertSpec;
use crate::capture::Captures;
use crate::kv::KeyValues;
use crate::vars::Variables;

/// The collection format version this crate reads and writes.
pub const FORMAT_VERSION: u32 = 1;

/// `snapreq.toml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollectionFile {
    pub format: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "Variables::is_empty")]
    pub vars: Variables,
    #[serde(default, skip_serializing_if = "KeyValues::is_empty")]
    pub headers: KeyValues,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSpec>,
    #[serde(default, skip_serializing_if = "Settings::is_empty")]
    pub settings: Settings,
}

impl CollectionFile {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            format: FORMAT_VERSION,
            name: name.into(),
            description: None,
            order: Vec::new(),
            vars: Variables::default(),
            headers: KeyValues::default(),
            auth: None,
            settings: Settings::default(),
        }
    }
}

/// `folder.toml` (optional in every folder).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FolderFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    #[serde(default, skip_serializing_if = "Variables::is_empty")]
    pub vars: Variables,
    #[serde(default, skip_serializing_if = "KeyValues::is_empty")]
    pub headers: KeyValues,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSpec>,
    #[serde(default, skip_serializing_if = "Settings::is_empty")]
    pub settings: Settings,
}

/// One request file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RequestFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// HTTP method; `None` means GET.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "method"
    )]
    pub method: Option<String>,
    pub url: String,
    #[serde(default, skip_serializing_if = "Variables::is_empty")]
    pub vars: Variables,
    #[serde(default, skip_serializing_if = "KeyValues::is_empty")]
    pub query: KeyValues,
    #[serde(default, skip_serializing_if = "KeyValues::is_empty")]
    pub headers: KeyValues,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<BodySpec>,
    #[serde(default, skip_serializing_if = "Settings::is_empty")]
    pub settings: Settings,
    /// Values taken from the response into variables for later requests.
    #[serde(default, skip_serializing_if = "Captures::is_empty")]
    pub capture: Captures,
    /// Checks run on the response (boxed: most requests have none, and it is large).
    #[serde(default, skip_serializing_if = "no_checks")]
    pub assert: Box<AssertSpec>,
}

impl RequestFile {
    pub fn new(method: impl Into<String>, url: impl Into<String>) -> Self {
        let method = method.into();
        Self {
            name: None,
            description: None,
            method: (!method.eq_ignore_ascii_case("GET")).then(|| method.to_ascii_uppercase()),
            url: url.into(),
            vars: Variables::default(),
            query: KeyValues::default(),
            headers: KeyValues::default(),
            auth: None,
            body: None,
            settings: Settings::default(),
            capture: Captures::default(),
            assert: Box::default(),
        }
    }

    /// The method, defaulting to GET.
    pub fn method(&self) -> &str {
        self.method.as_deref().unwrap_or("GET")
    }

    /// Checks rules serde cannot express. Returns the problem, if any.
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.url.trim().is_empty() {
            return Err("`url` must not be empty".into());
        }

        match &self.body {
            Some(BodySpec::Text { content, file, .. } | BodySpec::Json { content, file }) => {
                match (content, file) {
                    (Some(_), Some(_)) => {
                        return Err("body: set either `content` or `file`, not both".into());
                    }
                    (None, None) => return Err("body: set `content` or `file`".into()),
                    _ => {}
                }
            }
            Some(BodySpec::Multipart { parts }) => {
                for p in parts {
                    match (&p.value, &p.file) {
                        (Some(_), Some(_)) => {
                            return Err(format!(
                                "body part `{}`: set either `value` or `file`, not both",
                                p.name
                            ));
                        }
                        (None, None) => {
                            return Err(format!("body part `{}`: set `value` or `file`", p.name));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        self.capture.validate()?;
        self.settings.validate()
    }
}

fn method<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let raw = String::deserialize(d)?;
    let upper = raw.trim().to_ascii_uppercase();
    http::Method::from_bytes(upper.as_bytes())
        .map_err(|_| serde::de::Error::custom(format!("invalid HTTP method `{raw}`")))?;
    Ok(Some(upper))
}

/// An `[auth]` table. Missing means `inherit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuthSpec {
    /// Use the auth of the enclosing folder or collection.
    Inherit,
    /// No auth, even if the folder or collection has one.
    None,
    Basic {
        username: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        password: String,
    },
    Bearer {
        token: String,
    },
    Digest {
        username: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        password: String,
    },
    ApiKey {
        name: String,
        value: String,
        #[serde(default, skip_serializing_if = "is_header")]
        location: ApiKeyLocation,
    },
    #[serde(rename = "oauth2")]
    OAuth2(OAuth2Spec),
    #[serde(rename = "aws_sigv4")]
    AwsSigV4 {
        access_key: String,
        secret_key: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        session_token: String,
        region: String,
        service: String,
    },
}

/// `type = "oauth2"` in an `[auth]` table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OAuth2Spec {
    pub grant: OAuth2Grant,
    pub token_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_url: Option<String>,
    pub client_id: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub client_secret: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scope: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub username: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub password: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_auth: Option<ClientAuth>,
    /// Extra authorization and token request parameters (for example `audience`).
    #[serde(default, skip_serializing_if = "KeyValues::is_empty")]
    pub params: KeyValues,
}

impl AuthSpec {
    /// A one-line description without secrets, for listings: `bearer`,
    /// `oauth2 client_credentials`, `api_key header X-API-Key`, `aws_sigv4 eu-west-1/s3`.
    pub fn summary(&self) -> String {
        match self {
            Self::Inherit => "inherit".into(),
            Self::None => "none".into(),
            Self::Basic { .. } => "basic".into(),
            Self::Bearer { .. } => "bearer".into(),
            Self::Digest { .. } => "digest".into(),
            Self::ApiKey { name, location, .. } => format!(
                "api_key {} {name}",
                match location {
                    ApiKeyLocation::Header => "header",
                    ApiKeyLocation::Query => "query",
                }
            ),
            Self::OAuth2(o) => format!("oauth2 {}", o.grant.as_str()),
            Self::AwsSigV4 {
                region, service, ..
            } => format!("aws_sigv4 {region}/{service}"),
        }
    }
}

fn is_header(l: &ApiKeyLocation) -> bool {
    *l == ApiKeyLocation::Header
}

/// A request body in a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum BodySpec {
    Text {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<PathBuf>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    Json {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file: Option<PathBuf>,
    },
    Form {
        #[serde(default)]
        fields: KeyValues,
    },
    Multipart {
        #[serde(default)]
        parts: Vec<PartSpec>,
    },
    File {
        file: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

/// One multipart part in a file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PartSpec {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub filename: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_type: Option<String>,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

// serde's `skip_serializing_if` passes a reference to the field.
#[allow(clippy::borrowed_box)]
fn no_checks(a: &Box<AssertSpec>) -> bool {
    a.is_empty()
}

fn yes() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

/// Execution settings in a file; every key is optional and inherits when absent.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Settings {
    /// Total time limit in milliseconds; `0` means no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Connection time limit in milliseconds; `0` means no limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connect_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_redirects: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decompress: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookies: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_version: Option<HttpVersionPref>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub proxy: Option<ProxyMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_tls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_cert: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_cert: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_key: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_min_version: Option<TlsVersion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tls_max_version: Option<TlsVersion>,
}

impl Settings {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn validate(&self) -> Result<(), String> {
        if self.client_cert.is_some() != self.client_key.is_some() {
            return Err("settings: `client_cert` and `client_key` must be set together".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(text: &str) -> Result<RequestFile, String> {
        toml::from_str::<RequestFile>(text).map_err(|e| e.message().to_owned())
    }

    #[test]
    fn full_request_file() {
        let r = request(
            r#"
            name = "Create user"
            method = "post"
            url = "{{base_url}}/users"

            [headers]
            Authorization = "Bearer {{token}}"

            [body]
            type = "json"
            content = '{ "name": "snap" }'

            [settings]
            timeout_ms = 5000
            http_version = "http2"
            proxy = "socks5h://p:1080"
            tls_min_version = "1.3"
            "#,
        )
        .unwrap();
        assert_eq!(r.method(), "POST");
        assert_eq!(r.headers.0.len(), 1);
        assert!(matches!(
            r.body,
            Some(BodySpec::Json {
                content: Some(_),
                file: None
            })
        ));
        assert_eq!(r.settings.timeout_ms, Some(5000));
        assert_eq!(r.settings.http_version, Some(HttpVersionPref::Http2));
        assert_eq!(
            r.settings.proxy,
            Some(ProxyMode::Url("socks5h://p:1080".into()))
        );
        assert_eq!(r.settings.tls_min_version, Some(TlsVersion::Tls13));
        assert!(r.validate().is_ok());
    }

    #[test]
    fn minimal_request_defaults_to_get() {
        let r = request("url = \"https://a.test\"").unwrap();
        assert_eq!(r.method(), "GET");
        assert!(r.body.is_none() && r.settings.is_empty());
    }

    #[test]
    fn typos_and_bad_values_are_errors() {
        assert!(
            request("url = \"x\"\nmethd = \"GET\"")
                .unwrap_err()
                .contains("unknown field `methd`")
        );
        assert!(
            request("method = \"GE T\"\nurl = \"x\"")
                .unwrap_err()
                .contains("invalid HTTP method")
        );
        assert!(
            request("name = \"x\"")
                .unwrap_err()
                .contains("missing field `url`")
        );
        assert!(
            request("url = \"x\"\n[body]\ntype = \"yaml\"")
                .unwrap_err()
                .contains("unknown variant `yaml`")
        );
        assert!(
            request("url = \"x\"\n[body]\ntype = \"json\"\ncontnt = \"{}\"")
                .unwrap_err()
                .contains("unknown field `contnt`")
        );
        assert!(
            request("url = \"x\"\n[settings]\ntimeout = 5")
                .unwrap_err()
                .contains("unknown field `timeout`")
        );
    }

    #[test]
    fn semantic_validation() {
        let both =
            request("url = \"x\"\n[body]\ntype = \"text\"\ncontent = \"a\"\nfile = \"b.txt\"")
                .unwrap();
        assert_eq!(
            both.validate().unwrap_err(),
            "body: set either `content` or `file`, not both"
        );
        let neither = request("url = \"x\"\n[body]\ntype = \"json\"").unwrap();
        assert_eq!(
            neither.validate().unwrap_err(),
            "body: set `content` or `file`"
        );
        let part =
            request("url = \"x\"\n[body]\ntype = \"multipart\"\n[[body.parts]]\nname = \"p\"")
                .unwrap();
        assert_eq!(
            part.validate().unwrap_err(),
            "body part `p`: set `value` or `file`"
        );
        let cert = request("url = \"x\"\n[settings]\nclient_cert = \"a.crt\"").unwrap();
        assert!(
            cert.validate()
                .unwrap_err()
                .contains("must be set together")
        );
        assert!(request("url = \" \"").unwrap().validate().is_err());
    }
}
