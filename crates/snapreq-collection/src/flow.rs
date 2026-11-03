//! Flow files (`*.flow.toml`): a workflow of chained requests (see `docs/FORMAT.md`).
//!
//! ```toml
//! name = "Checkout"
//!
//! [vars]
//! item = "sku-42"
//!
//! [[step]]
//! request = "auth/login"             # a request of the collection
//! [step.capture]
//! token = { json = "$.access_token", secret = true }
//!
//! [[step]]
//! name = "Create order"              # or an inline request
//! method = "POST"
//! url = "{{base_url}}/orders"
//! [step.body]
//! type = "json"
//! content = '{ "item": "{{item}}" }'
//! [step.assert]
//! status = 201
//!
//! [[step]]
//! request = "orders/get"
//! retry = { max = 20, every_ms = 500 }   # until the step's checks pass
//! ```

use serde::{Deserialize, Serialize};

use crate::assert::AssertSpec;
use crate::capture::Captures;
use crate::files::{AuthSpec, BodySpec, RequestFile, Settings};
use crate::kv::KeyValues;
use crate::vars::Variables;

/// File name suffix of flows.
pub const FLOW_SUFFIX: &str = ".flow.toml";

/// A `*.flow.toml` file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlowFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Defaults for every step; captured values and step variables override them.
    #[serde(default, skip_serializing_if = "Variables::is_empty")]
    pub vars: Variables,
    #[serde(default, rename = "step")]
    pub steps: Vec<StepSpec>,
}

/// One step: a collection request (`request = "key"`) or an inline request (`url = ...`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StepSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Key of a request in the collection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,

    // Inline request (same meaning as in request files).
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "crate::files::method"
    )]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
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

    /// Variables for this step only (above captured values).
    #[serde(default, skip_serializing_if = "Variables::is_empty")]
    pub vars: Variables,
    /// Wait this long before the step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delay_ms: Option<u64>,
    /// Send again until a check passes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<RetrySpec>,
    #[serde(default, skip_serializing_if = "Captures::is_empty")]
    pub capture: Captures,
    #[serde(default, skip_serializing_if = "no_checks")]
    pub assert: Box<AssertSpec>,
}

#[allow(clippy::borrowed_box)] // serde passes a reference to the field
fn no_checks(a: &Box<AssertSpec>) -> bool {
    a.is_empty()
}

/// `retry = { max, every_ms }` with an optional `[step.retry.until]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetrySpec {
    /// Attempts in total, at least 1.
    #[serde(default = "default_max")]
    pub max: u32,
    /// Pause between attempts.
    #[serde(default = "default_every")]
    pub every_ms: u64,
    /// The checks that end the retries. Default: the step's own checks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub until: Option<Box<AssertSpec>>,
}

fn default_max() -> u32 {
    10
}

fn default_every() -> u64 {
    1000
}

impl StepSpec {
    /// The step's title: its name, the request key, or `METHOD url`.
    pub fn title(&self) -> String {
        if let Some(n) = &self.name {
            return n.clone();
        }
        if let Some(r) = &self.request {
            return r.clone();
        }
        format!(
            "{} {}",
            self.method.as_deref().unwrap_or("GET"),
            self.url.as_deref().unwrap_or_default()
        )
    }

    /// The inline request as a request file, or `None` for a `request = ...` step.
    pub fn inline_request(&self) -> Option<RequestFile> {
        let url = self.url.clone()?;
        let mut f = RequestFile::new(self.method.as_deref().unwrap_or("GET"), url);
        f.query = self.query.clone();
        f.headers = self.headers.clone();
        f.auth = self.auth.clone();
        f.body = self.body.clone();
        f.settings = self.settings.clone();
        Some(f)
    }

    fn validate(&self) -> Result<(), String> {
        let inline_fields = self.method.is_some()
            || !self.query.is_empty()
            || !self.headers.is_empty()
            || self.auth.is_some()
            || self.body.is_some()
            || !self.settings.is_empty();
        match (&self.request, &self.url) {
            (Some(_), Some(_)) => return Err("set either `request` or `url`, not both".into()),
            (None, None) => {
                return Err(
                    "set `request` (a collection request) or `url` (an inline request)".into(),
                );
            }
            (Some(_), None) if inline_fields => {
                return Err(
                    "a `request` step cannot set method, query, headers, auth, body or settings; use an inline step (`url = ...`) or change the request file".into(),
                );
            }
            _ => {}
        }
        if let Some(f) = self.inline_request() {
            f.validate()?;
        }
        if let Some(r) = &self.retry
            && r.max == 0
        {
            return Err("retry: `max` must be at least 1".into());
        }
        self.capture.validate()
    }
}

impl FlowFile {
    /// Checks rules serde cannot express; the message names the step.
    pub fn validate(&self) -> Result<(), String> {
        if self.steps.is_empty() {
            return Err("a flow needs at least one `[[step]]`".into());
        }
        for (i, step) in self.steps.iter().enumerate() {
            step.validate()
                .map_err(|e| format!("step {} ({}): {e}", i + 1, step.title()))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<FlowFile, String> {
        let f: FlowFile = toml::from_str(text).map_err(|e| e.message().to_owned())?;
        f.validate()?;
        Ok(f)
    }

    #[test]
    fn steps_of_both_kinds() {
        let f = parse(
            r#"
            name = "Checkout"
            [vars]
            item = "sku-42"

            [[step]]
            request = "auth/login"
            [step.capture]
            token = { json = "$.t", secret = true }

            [[step]]
            method = "post"
            url = "{{base}}/orders"
            [step.headers]
            X-Idempotency = "{{$uuid}}"
            [step.body]
            type = "json"
            content = "{}"
            [step.assert]
            status = 201

            [[step]]
            request = "orders/get"
            delay_ms = 100
            [step.retry]
            max = 5
            every_ms = 200
            [step.retry.until.json]
            "$.state" = "paid"
            "#,
        )
        .unwrap();
        assert_eq!(f.steps.len(), 3);
        assert_eq!(f.steps[0].title(), "auth/login");
        assert!(f.steps[0].inline_request().is_none());
        let inline = f.steps[1].inline_request().unwrap();
        assert_eq!(inline.method(), "POST");
        assert_eq!(f.steps[1].title(), "POST {{base}}/orders");
        let retry = f.steps[2].retry.as_ref().unwrap();
        assert_eq!((retry.max, retry.every_ms), (5, 200));
        assert_eq!(retry.until.as_ref().unwrap().json.0.len(), 1);
    }

    #[test]
    fn invalid_flows() {
        for (text, needle) in [
            ("name = \"x\"", "at least one `[[step]]`"),
            ("[[step]]\nname = \"a\"", "step 1 (a): set `request`"),
            ("[[step]]\nrequest = \"a\"\nurl = \"b\"", "not both"),
            (
                "[[step]]\nrequest = \"a\"\nmethod = \"POST\"",
                "cannot set method",
            ),
            ("[[step]]\nurl = \"x\"\nretry = { max = 0 }", "at least 1"),
            (
                "[[step]]\nurl = \"x\"\n[step.body]\ntype = \"json\"",
                "set `content` or `file`",
            ),
            (
                "[[step]]\nurl = \"x\"\nmethod = \"GE T\"",
                "invalid HTTP method",
            ),
            (
                "[[step]]\nurl = \"x\"\nretries = 3",
                "unknown field `retries`",
            ),
        ] {
            let err = parse(text).unwrap_err();
            assert!(err.contains(needle), "{text}: {err}");
        }
    }
}
