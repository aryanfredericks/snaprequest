//! The `[capture]` table: values taken from a response into variables for later requests.
//!
//! ```toml
//! [capture]
//! user_id = "$.id"                                   # a plain string is a JSONPath
//! token = { json = "$.access_token", secret = true }
//! location = { header = "location" }
//! session = { cookie = "sid" }
//! csrf = { regex = 'name="csrf" value="([^"]+)"' }    # group 1 if the pattern has groups
//! code = { status = true }
//! ```
//!
//! `snapreq-test` does the extraction.

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};

/// Where one captured value comes from. Exactly one source must be set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSpec {
    /// JSONPath into the body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub json: Option<String>,
    /// Response header (case-insensitive; the first value).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub header: Option<String>,
    /// Cookie set by the response (`Set-Cookie`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cookie: Option<String>,
    /// Regular expression over the body text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub regex: Option<String>,
    /// Capture group of `regex`; default 1 if the pattern has groups, else the whole match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<usize>,
    /// The status code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<bool>,
    /// The whole body as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<bool>,
    /// Hide the value in every later trace and report.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub secret: bool,
}

/// The source of a capture, after validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureSource<'a> {
    Json(&'a str),
    Header(&'a str),
    Cookie(&'a str),
    Regex(&'a str, Option<usize>),
    Status,
    Body,
}

impl CaptureSpec {
    /// The one source that is set, or why the spec is invalid.
    pub fn source(&self) -> Result<CaptureSource<'_>, String> {
        let mut found = Vec::new();
        if let Some(p) = &self.json {
            found.push(CaptureSource::Json(p));
        }
        if let Some(h) = &self.header {
            found.push(CaptureSource::Header(h));
        }
        if let Some(c) = &self.cookie {
            found.push(CaptureSource::Cookie(c));
        }
        if let Some(r) = &self.regex {
            found.push(CaptureSource::Regex(r, self.group));
        }
        if self.status == Some(true) {
            found.push(CaptureSource::Status);
        }
        if self.body == Some(true) {
            found.push(CaptureSource::Body);
        }
        if self.group.is_some() && self.regex.is_none() {
            return Err("`group` only works with `regex`".into());
        }
        match found.as_slice() {
            [one] => Ok(*one),
            [] => Err("set one of `json`, `header`, `cookie`, `regex`, `status` or `body`".into()),
            _ => Err(
                "set only one of `json`, `header`, `cookie`, `regex`, `status` and `body`".into(),
            ),
        }
    }

    /// The JSONPath shorthand form, if this spec can be written as a plain string.
    fn shorthand(&self) -> Option<&str> {
        let json = self.json.as_deref()?;
        (Self {
            json: None,
            ..self.clone()
        } == Self::default())
        .then_some(json)
    }
}

/// Ordered `name = capture` entries.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Captures(pub Vec<(String, CaptureSpec)>);

impl Captures {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Checks names and sources; returns the first problem.
    pub fn validate(&self) -> Result<(), String> {
        for (name, spec) in &self.0 {
            if !crate::vars::valid_name(name) {
                return Err(format!(
                    "capture `{name}`: invalid variable name (letters, digits, `_`, `-`, `.`)"
                ));
            }
            spec.source()
                .map_err(|e| format!("capture `{name}`: {e}"))?;
        }
        Ok(())
    }
}

impl Serialize for Captures {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            match v.shorthand() {
                Some(path) => map.serialize_entry(k, path)?,
                None => map.serialize_entry(k, v)?,
            }
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Captures {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Captures;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a table of captures")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Captures, A::Error> {
                let mut out = Vec::new();
                while let Some(k) = m.next_key::<String>()? {
                    let v: toml::Value = m.next_value()?;
                    let spec = match v {
                        toml::Value::String(path) => CaptureSpec {
                            json: Some(path),
                            ..CaptureSpec::default()
                        },
                        other => other.try_into::<CaptureSpec>().map_err(|e| {
                            de::Error::custom(format!("capture `{k}`: {}", e.message()))
                        })?,
                    };
                    out.push((k, spec));
                }
                Ok(Captures(out))
            }
        }
        d.deserialize_map(V)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<Captures, String> {
        #[derive(Deserialize)]
        struct F {
            capture: Captures,
        }
        toml::from_str::<F>(text)
            .map(|f| f.capture)
            .map_err(|e| e.message().to_owned())
    }

    #[test]
    fn forms_and_validation() {
        let c = parse(
            "[capture]\nid = \"$.id\"\ntoken = { json = \"$.t\", secret = true }\nloc = { header = \"Location\" }\ncsrf = { regex = 'v=(\\w+)', group = 1 }\ncode = { status = true }\n",
        )
        .unwrap();
        assert!(c.validate().is_ok());
        let names: Vec<&str> = c.0.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["id", "token", "loc", "csrf", "code"]);
        assert_eq!(c.0[0].1.source(), Ok(CaptureSource::Json("$.id")));
        assert!(c.0[1].1.secret);
        assert_eq!(
            c.0[3].1.source(),
            Ok(CaptureSource::Regex("v=(\\w+)", Some(1)))
        );
        assert_eq!(c.0[4].1.source(), Ok(CaptureSource::Status));

        for (text, needle) in [
            ("x = { header = \"a\", cookie = \"b\" }", "set only one"),
            ("x = { secret = true }", "set one of"),
            ("x = { json = \"$.a\", group = 2 }", "`group` only works"),
            ("\"bad name\" = \"$.a\"", "invalid variable name"),
        ] {
            let err = parse(&format!("[capture]\n{text}\n"))
                .unwrap()
                .validate()
                .unwrap_err();
            assert!(err.contains(needle), "{text}: {err}");
        }
        let err = parse("[capture]\nx = { jsn = \"$.a\" }\n").unwrap_err();
        assert!(err.contains("capture `x`: unknown field `jsn`"), "{err}");
    }
}
