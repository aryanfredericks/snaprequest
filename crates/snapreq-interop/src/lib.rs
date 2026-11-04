//! Moving requests between SnapRequest and other tools (PLAN Phase 9, ADR-029).
//!
//! [`import`] turns a cURL command, a Postman collection (v2.1, with environments), an OpenAPI 3.x
//! document, an Insomnia export (v4 JSON or v5 YAML), a HAR recording or a Hurl file into an
//! [`Imported`] collection, which [`Imported::write`] saves as collection files.
//!
//! Credentials found in the source (passwords, tokens, API keys, cookies) never land in collection
//! files: they become secret variables whose values are stored in the collection's `.secrets`
//! directory (sealed when a key is given).

mod curl;
mod error;
mod har;
mod hurl;
mod insomnia;
mod model;
mod openapi;
mod postman;
mod util;

use std::fmt;
use std::str::FromStr;

use serde_json::Value;

pub use error::{Error, Result};
pub use model::{Imported, Node, Summary};

/// A file-name-friendly key for a name (lower-case letters, digits and `-`).
pub fn slug(name: &str, fallback: &str) -> String {
    util::slug(name, fallback)
}

/// A format [`import`] reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// One or more `curl` commands.
    Curl,
    /// Postman Collection v2.1 (v2.0 is read too).
    Postman,
    /// A Postman environment, imported together with a collection.
    PostmanEnvironment,
    /// OpenAPI 3.0 or 3.1, JSON or YAML.
    OpenApi,
    /// Insomnia export: v4 JSON or v5 YAML.
    Insomnia,
    /// HTTP Archive 1.2.
    Har,
    /// A Hurl file, imported as a flow.
    Hurl,
}

impl Format {
    pub const ALL: [Self; 7] = [
        Self::Curl,
        Self::Postman,
        Self::PostmanEnvironment,
        Self::OpenApi,
        Self::Insomnia,
        Self::Har,
        Self::Hurl,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Curl => "curl",
            Self::Postman => "postman",
            Self::PostmanEnvironment => "postman-environment",
            Self::OpenApi => "openapi",
            Self::Insomnia => "insomnia",
            Self::Har => "har",
            Self::Hurl => "hurl",
        }
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Format {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, String> {
        Self::ALL
            .into_iter()
            .find(|f| f.as_str() == s)
            .ok_or_else(|| {
                format!(
                    "unknown format `{s}` (expected one of: {})",
                    Self::ALL.map(Self::as_str).join(", ")
                )
            })
    }
}

/// Inputs larger than this are refused (64 MiB).
pub const MAX_INPUT: usize = 64 << 20;

/// Parses JSON, or YAML when the text is not JSON.
pub(crate) fn parse_document(text: &str) -> std::result::Result<Value, String> {
    match serde_json::from_str(text) {
        Ok(v) => Ok(v),
        Err(json) => {
            let first = text.trim_start().chars().next();
            if matches!(first, Some('{' | '[')) {
                return Err(format!("invalid JSON: {json}"));
            }
            serde_saphyr::from_str::<Value>(text).map_err(|e| format!("invalid YAML: {e}"))
        }
    }
}

/// Recognises the format of `text`.
pub fn detect(text: &str) -> Result<Format> {
    let trimmed = text.trim_start_matches('\u{feff}').trim_start();
    if curl::looks_like(trimmed) {
        return Ok(Format::Curl);
    }
    if hurl::looks_like(trimmed) {
        return Ok(Format::Hurl);
    }
    let doc = parse_document(trimmed).map_err(|e| Error::Unrecognized(e.to_string()))?;
    if doc.get("openapi").is_some() {
        return Ok(Format::OpenApi);
    }
    if doc.get("swagger").is_some() {
        return Err(Error::Unsupported(
            "Swagger 2.0 is not supported; convert it to OpenAPI 3 first (for example with swagger2openapi)".into(),
        ));
    }
    if doc["log"]["entries"].is_array() {
        return Ok(Format::Har);
    }
    if doc["_type"] == "export"
        || doc["type"]
            .as_str()
            .is_some_and(|t| t.starts_with("collection.insomnia.rest/"))
    {
        return Ok(Format::Insomnia);
    }
    if doc["info"]["schema"]
        .as_str()
        .is_some_and(|s| s.contains("getpostman.com") || s.contains("postman"))
        || (doc["info"].is_object() && doc["item"].is_array())
    {
        return Ok(Format::Postman);
    }
    if doc["values"].is_array()
        && (doc.get("_postman_variable_scope").is_some() || doc["name"].is_string())
    {
        return Ok(Format::PostmanEnvironment);
    }
    Err(Error::Unrecognized(
        "not a cURL command, Postman collection or environment, OpenAPI document, Insomnia export, HAR or Hurl file".into(),
    ))
}

/// Options for [`import`].
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The format; detected when `None`.
    pub format: Option<Format>,
    /// The collection's name; otherwise taken from the source (or "Imported").
    pub name: Option<String>,
}

/// Imports `text` (after [`detect`]ing its format unless given).
pub fn import(text: &str, options: &Options) -> Result<Imported> {
    if text.len() > MAX_INPUT {
        return Err(Error::Unsupported(format!(
            "the input is larger than {} MiB",
            MAX_INPUT >> 20
        )));
    }
    let text = text.trim_start_matches('\u{feff}');
    let format = match options.format {
        Some(f) => f,
        None => detect(text)?,
    };
    let mut imported = match format {
        Format::Curl => curl::import(text)?,
        Format::Postman => postman::import(text)?,
        Format::PostmanEnvironment => {
            return Err(Error::Unsupported(
                "a Postman environment is imported together with its collection".into(),
            ));
        }
        Format::OpenApi => openapi::import(text)?,
        Format::Insomnia => insomnia::import(text)?,
        Format::Har => har::import(text)?,
        Format::Hurl => hurl::import(text)?,
    };
    if let Some(name) = &options.name {
        imported.collection.name.clone_from(name);
        if format == Format::Hurl {
            for item in &mut imported.items {
                if let Node::Flow { file, .. } = item {
                    file.name = Some(name.clone());
                }
            }
        }
    }
    imported.finish();
    Ok(imported)
}

/// Adds a Postman environment file to a collection imported from Postman (or any other source).
pub fn add_postman_environment(imported: &mut Imported, text: &str) -> Result<String> {
    let (name, vars) = postman::environment(text)?;
    let name = imported.add_environment(&name, vars);
    imported.finish();
    Ok(name)
}
