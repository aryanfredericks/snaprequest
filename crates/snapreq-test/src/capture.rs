//! Extracting `[capture]` values from a response.

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use serde_json_path::JsonPath;
use snapreq_collection::{CaptureSource, Captures};
use snapreq_core::Response;

/// One captured value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Captured {
    pub name: String,
    /// `None` when the source was missing; `error` says why.
    pub value: Option<String>,
    pub secret: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Captured {
    /// The value for display: secret values are replaced.
    pub fn shown(&self) -> Option<&str> {
        match (&self.value, self.secret) {
            (Some(_), true) => Some("<redacted>"),
            (v, _) => v.as_deref(),
        }
    }
}

enum Source {
    Json(String, JsonPath),
    Header(String),
    Cookie(String),
    Regex(Regex, Option<usize>),
    Status,
    Body,
}

/// Compiled captures of one request.
pub struct Capturer {
    items: Vec<(String, Source, bool)>,
}

impl std::fmt::Debug for Capturer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Capturer")
            .field("captures", &self.items.len())
            .finish()
    }
}

impl Capturer {
    /// Validates sources, JSONPaths and patterns before sending.
    pub fn compile(captures: &Captures) -> Result<Self, String> {
        captures.validate()?;
        let mut items = Vec::new();
        for (name, spec) in &captures.0 {
            let place = format!("capture `{name}`");
            let source = match spec.source().map_err(|e| format!("{place}: {e}"))? {
                CaptureSource::Json(p) => Source::Json(
                    p.to_owned(),
                    JsonPath::parse(p)
                        .map_err(|e| format!("{place}: invalid JSONPath `{p}`: {e}"))?,
                ),
                CaptureSource::Header(h) => Source::Header(h.trim().to_owned()),
                CaptureSource::Cookie(c) => Source::Cookie(c.trim().to_owned()),
                CaptureSource::Regex(r, group) => Source::Regex(
                    Regex::new(r)
                        .map_err(|e| format!("{place}: invalid regular expression: {e}"))?,
                    group,
                ),
                CaptureSource::Status => Source::Status,
                CaptureSource::Body => Source::Body,
            };
            items.push((name.clone(), source, spec.secret));
        }
        Ok(Self { items })
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn extract(&self, resp: &Response) -> Vec<Captured> {
        let body = String::from_utf8_lossy(&resp.body);
        let json = std::cell::OnceCell::new();
        self.items
            .iter()
            .map(|(name, source, secret)| {
                let result: Result<String, String> = match source {
                    Source::Json(text, path) => json
                        .get_or_init(|| {
                            serde_json::from_slice::<Value>(&resp.body)
                                .map_err(|e| format!("the body is not JSON: {e}"))
                        })
                        .as_ref()
                        .map_err(Clone::clone)
                        .and_then(|doc| {
                            let nodes = path.query(doc).all();
                            match nodes.as_slice() {
                                [] => Err(format!("`{text}` matched nothing")),
                                [one] => Ok(as_text(one)),
                                many => {
                                    Ok(Value::Array(many.iter().map(|v| (*v).clone()).collect())
                                        .to_string())
                                }
                            }
                        }),
                    Source::Header(h) => resp
                        .header(h)
                        .map(str::to_owned)
                        .ok_or_else(|| format!("no `{h}` header")),
                    Source::Cookie(c) => resp
                        .headers
                        .iter()
                        .filter(|h| h.name.eq_ignore_ascii_case("set-cookie"))
                        .filter_map(|h| h.value.split(';').next()?.split_once('='))
                        .find(|(n, _)| n.trim() == c)
                        .map(|(_, v)| v.trim().to_owned())
                        .ok_or_else(|| format!("no `{c}` cookie was set")),
                    Source::Regex(re, group) => match re.captures(&body) {
                        None => Err(format!("/{}/ did not match the body", re.as_str())),
                        Some(caps) => {
                            let index = group.unwrap_or(usize::from(caps.len() > 1));
                            caps.get(index)
                                .map(|m| m.as_str().to_owned())
                                .ok_or_else(|| format!("/{}/ has no group {index}", re.as_str()))
                        }
                    },
                    Source::Status => Ok(resp.status.to_string()),
                    Source::Body => Ok(body.clone().into_owned()),
                };
                match result {
                    Ok(value) => Captured {
                        name: name.clone(),
                        value: Some(value),
                        secret: *secret,
                        error: None,
                    },
                    Err(e) => Captured {
                        name: name.clone(),
                        value: None,
                        secret: *secret,
                        error: Some(e),
                    },
                }
            })
            .collect()
    }
}

/// Strings as themselves, anything else as compact JSON.
fn as_text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
