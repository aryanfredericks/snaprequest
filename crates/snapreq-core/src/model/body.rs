//! Request bodies and key/value entries.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A name/value pair that can be switched off without deleting it.
/// Used for headers, query parameters and form fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub name: String,
    pub value: String,
    #[serde(default = "enabled_default", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

fn enabled_default() -> bool {
    true
}

fn is_true(b: &bool) -> bool {
    *b
}

impl KeyValue {
    /// An enabled entry.
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
        }
    }

    /// Returns the same entry, disabled.
    #[must_use]
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }
}

/// The request body.
///
/// Serialized with a `type` tag, e.g. `{ "type": "json", "content": "{...}" }`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Body {
    /// No body.
    #[default]
    None,
    /// Arbitrary text. `content_type` defaults to `text/plain; charset=utf-8`.
    Text {
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    /// JSON text, sent as-is with `Content-Type: application/json`.
    /// Kept as text so the user's formatting is preserved.
    Json { content: String },
    /// `application/x-www-form-urlencoded` fields.
    Form { fields: Vec<KeyValue> },
    /// `multipart/form-data` parts.
    Multipart { parts: Vec<MultipartPart> },
    /// A GraphQL operation, sent as JSON `{"query", "variables", "operationName"}` (PLAN P10.3).
    Graphql {
        query: String,
        /// A JSON object, as text.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        variables: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_name: Option<String>,
    },
    /// The contents of a file, streamed from disk.
    File {
        path: PathBuf,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

impl Body {
    /// `true` for [`Body::None`].
    pub fn is_none(&self) -> bool {
        matches!(self, Self::None)
    }

    /// The `Content-Type` this body sends unless the user sets one explicitly.
    ///
    /// Multipart returns `None` because its header needs the boundary, which is
    /// chosen when the body is encoded. A file with no explicit type gets one
    /// guessed from its extension, falling back to `application/octet-stream`.
    pub fn default_content_type(&self) -> Option<&str> {
        match self {
            Self::None | Self::Multipart { .. } => None,
            Self::Text { content_type, .. } => Some(
                content_type
                    .as_deref()
                    .unwrap_or("text/plain; charset=utf-8"),
            ),
            Self::Json { .. } | Self::Graphql { .. } => Some("application/json"),
            Self::Form { .. } => Some("application/x-www-form-urlencoded"),
            Self::File { path, content_type } => Some(
                content_type
                    .as_deref()
                    .unwrap_or_else(|| guess_content_type(path)),
            ),
        }
    }
}

/// The JSON document of a GraphQL request. `variables` must be a JSON object (or empty).
pub fn graphql_document(
    query: &str,
    variables: Option<&str>,
    operation_name: Option<&str>,
) -> Result<String, String> {
    let mut doc = serde_json::Map::new();
    doc.insert("query".into(), serde_json::Value::String(query.to_owned()));
    if let Some(v) = variables.map(str::trim).filter(|v| !v.is_empty()) {
        let parsed: serde_json::Value = serde_json::from_str(v)
            .map_err(|e| format!("GraphQL variables are not valid JSON: {e}"))?;
        if !parsed.is_object() && !parsed.is_null() {
            return Err("GraphQL variables must be a JSON object".into());
        }
        doc.insert("variables".into(), parsed);
    }
    if let Some(op) = operation_name.map(str::trim).filter(|o| !o.is_empty()) {
        doc.insert(
            "operationName".into(),
            serde_json::Value::String(op.to_owned()),
        );
    }
    Ok(serde_json::Value::Object(doc).to_string())
}

impl Body {
    /// The text sent for text, JSON and GraphQL bodies (GraphQL: its JSON document, or the
    /// problem with its variables).
    pub fn text(&self) -> Option<Result<String, String>> {
        match self {
            Self::Text { content, .. } | Self::Json { content } => Some(Ok(content.clone())),
            Self::Graphql {
                query,
                variables,
                operation_name,
            } => Some(graphql_document(
                query,
                variables.as_deref(),
                operation_name.as_deref(),
            )),
            _ => None,
        }
    }
}

/// Content type for a file, guessed from its extension.
pub(crate) fn guess_content_type(path: &std::path::Path) -> &'static str {
    mime_guess::from_path(path)
        .first_raw()
        .unwrap_or("application/octet-stream")
}

/// One part of a `multipart/form-data` body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultipartPart {
    pub name: String,
    #[serde(flatten)]
    pub value: PartValue,
    #[serde(default = "enabled_default", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

/// The content of a multipart part.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PartValue {
    /// A plain text field.
    Text { value: String },
    /// A file upload.
    File {
        path: PathBuf,
        /// File name sent to the server; defaults to the file name of `path`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        filename: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn body_json_shape() {
        let body = Body::Json {
            content: r#"{"a":1}"#.into(),
        };
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({ "type": "json", "content": "{\"a\":1}" })
        );
    }

    #[test]
    fn multipart_round_trip() {
        let body = Body::Multipart {
            parts: vec![
                MultipartPart {
                    name: "title".into(),
                    value: PartValue::Text {
                        value: "hello".into(),
                    },
                    enabled: true,
                },
                MultipartPart {
                    name: "avatar".into(),
                    value: PartValue::File {
                        path: "img/me.png".into(),
                        filename: None,
                        content_type: Some("image/png".into()),
                    },
                    enabled: false,
                },
            ],
        };
        let value = serde_json::to_value(&body).unwrap();
        assert_eq!(
            value,
            json!({
                "type": "multipart",
                "parts": [
                    { "name": "title", "kind": "text", "value": "hello" },
                    { "name": "avatar", "kind": "file", "path": "img/me.png",
                      "content_type": "image/png", "enabled": false }
                ]
            })
        );
        assert_eq!(serde_json::from_value::<Body>(value).unwrap(), body);
    }

    #[test]
    fn key_value_enabled_defaults_to_true() {
        let kv: KeyValue = serde_json::from_value(json!({ "name": "a", "value": "b" })).unwrap();
        assert!(kv.enabled);
        assert_eq!(
            serde_json::to_value(&kv).unwrap(),
            json!({ "name": "a", "value": "b" })
        );
    }

    #[test]
    fn default_content_types() {
        assert_eq!(Body::None.default_content_type(), None);
        assert_eq!(
            Body::Json {
                content: "{}".into()
            }
            .default_content_type(),
            Some("application/json")
        );
        assert_eq!(
            Body::Text {
                content: String::new(),
                content_type: Some("text/csv".into())
            }
            .default_content_type(),
            Some("text/csv")
        );
        assert_eq!(
            Body::File {
                path: "x.bin".into(),
                content_type: None
            }
            .default_content_type(),
            Some("application/octet-stream")
        );
        assert_eq!(
            Body::File {
                path: "data/users.json".into(),
                content_type: None
            }
            .default_content_type(),
            Some("application/json")
        );
        assert_eq!(
            Body::Multipart { parts: vec![] }.default_content_type(),
            None
        );
    }
}
