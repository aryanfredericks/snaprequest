//! The `[assert]` table of request files (and, later, flow steps).
//!
//! ```toml
//! [assert]
//! status = 201                  # or "2xx", or [200, 201]
//! duration_ms = { lt = 800 }
//! body = { contains = "snap" }
//! schema = "schemas/user.json"
//!
//! [assert.headers]
//! content-type = { contains = "application/json" }
//!
//! [assert.json]
//! "$.id" = { type = "integer" }
//! "$.name" = "snap"             # a plain value means `equals`
//! ```
//!
//! This module only describes the checks; `snapreq-test` evaluates them.

use std::path::PathBuf;

use serde::de::{self, Deserializer, MapAccess, Visitor};
use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Every check of one request, in file order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssertSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<Check>,
    /// Total time of the exchange in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<Check>,
    /// The body as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Check>,
    /// JSON Schema file the body must validate against (relative to the request file).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema: Option<PathBuf>,
    /// Header name (case-insensitive) → check.
    #[serde(default, skip_serializing_if = "Checks::is_empty")]
    pub headers: Checks,
    /// JSONPath (RFC 9535) → check.
    #[serde(default, skip_serializing_if = "Checks::is_empty")]
    pub json: Checks,
}

impl AssertSpec {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// Applies `f` to every string a user may write placeholders in (expected values and
    /// the schema path); used for interpolation.
    pub fn try_map_strings<E>(
        &mut self,
        mut f: impl FnMut(&str, &str) -> Result<String, E>,
    ) -> Result<(), E> {
        for (place, check) in [
            ("assert.status", self.status.as_mut()),
            ("assert.duration_ms", self.duration_ms.as_mut()),
            ("assert.body", self.body.as_mut()),
        ] {
            if let Some(c) = check {
                c.try_map_strings(place, &mut f)?;
            }
        }
        if let Some(p) = &self.schema {
            self.schema = Some(PathBuf::from(f("assert.schema", &p.to_string_lossy())?));
        }
        for (section, checks) in [("headers", &mut self.headers), ("json", &mut self.json)] {
            for (key, c) in &mut checks.0 {
                c.try_map_strings(&format!("assert.{section}.{key}"), &mut f)?;
            }
        }
        Ok(())
    }
}

/// Ordered `key = check` entries (`[assert.headers]`, `[assert.json]`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Checks(pub Vec<(String, Check)>);

impl Checks {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

impl Serialize for Checks {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for (k, v) in &self.0 {
            map.serialize_entry(k, v)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Checks {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Checks;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a table of checks")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Checks, A::Error> {
                let mut out = Vec::new();
                while let Some((k, v)) = m.next_entry::<String, Check>()? {
                    out.push((k, v));
                }
                Ok(Checks(out))
            }
        }
        d.deserialize_map(V)
    }
}

/// JSON types for `type = "..."`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JsonType {
    String,
    Number,
    /// A number without a fractional part.
    Integer,
    Boolean,
    Array,
    Object,
    Null,
}

impl JsonType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Integer => "integer",
            Self::Boolean => "boolean",
            Self::Array => "array",
            Self::Object => "object",
            Self::Null => "null",
        }
    }
}

/// One subject's checks: every operator that is set must pass.
///
/// In a file a plain value is shorthand for `{ equals = value }`; a table lists operators.
/// To compare with an object, write `{ equals = { ... } }`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Check {
    pub equals: Option<Value>,
    pub not_equals: Option<Value>,
    /// Substring of a string, or element of an array.
    pub contains: Option<Value>,
    pub not_contains: Option<Value>,
    pub starts_with: Option<String>,
    pub ends_with: Option<String>,
    /// Regular expression (Rust `regex` syntax) that must match somewhere.
    pub matches: Option<String>,
    pub exists: Option<bool>,
    pub type_: Option<JsonType>,
    /// Characters of a string, elements of an array, or keys of an object.
    pub length: Option<u64>,
    pub lt: Option<serde_json::Number>,
    pub lte: Option<serde_json::Number>,
    pub gt: Option<serde_json::Number>,
    pub gte: Option<serde_json::Number>,
    pub one_of: Option<Vec<Value>>,
}

impl Check {
    /// `{ equals = value }`.
    pub fn equals(value: impl Into<Value>) -> Self {
        Self {
            equals: Some(value.into()),
            ..Self::default()
        }
    }

    fn try_map_strings<E>(
        &mut self,
        place: &str,
        f: &mut impl FnMut(&str, &str) -> Result<String, E>,
    ) -> Result<(), E> {
        for v in [
            &mut self.equals,
            &mut self.not_equals,
            &mut self.contains,
            &mut self.not_contains,
        ]
        .into_iter()
        .flatten()
        {
            map_value(v, place, f)?;
        }
        for s in [
            &mut self.starts_with,
            &mut self.ends_with,
            &mut self.matches,
        ]
        .into_iter()
        .flatten()
        {
            *s = f(place, s)?;
        }
        for v in self.one_of.iter_mut().flatten() {
            map_value(v, place, f)?;
        }
        Ok(())
    }
}

fn map_value<E>(
    v: &mut Value,
    place: &str,
    f: &mut impl FnMut(&str, &str) -> Result<String, E>,
) -> Result<(), E> {
    match v {
        Value::String(s) => *s = f(place, s)?,
        Value::Array(a) => {
            for x in a {
                map_value(x, place, f)?;
            }
        }
        Value::Object(o) => {
            for x in o.values_mut() {
                map_value(x, place, f)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// The full (table) form of [`Check`].
#[derive(Serialize, Deserialize)]
#[serde(remote = "Check", deny_unknown_fields)]
struct CheckDef {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    equals: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    not_equals: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    contains: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    not_contains: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    starts_with: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    ends_with: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    matches: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    exists: Option<bool>,
    #[serde(default, rename = "type", skip_serializing_if = "Option::is_none")]
    type_: Option<JsonType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    length: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lt: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lte: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gt: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gte: Option<serde_json::Number>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    one_of: Option<Vec<Value>>,
}

impl Check {
    /// Only `equals`, with a value that is not a table: written in shorthand.
    fn is_shorthand(&self) -> bool {
        matches!(&self.equals, Some(v) if !v.is_object())
            && Self {
                equals: None,
                ..self.clone()
            } == Self::default()
    }
}

impl Serialize for Check {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match &self.equals {
            Some(v) if self.is_shorthand() => v.serialize(s),
            _ => CheckDef::serialize(self, s),
        }
    }
}

impl<'de> Deserialize<'de> for Check {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        match Value::deserialize(d)? {
            Value::Object(map) => CheckDef::deserialize(Value::Object(map)).map_err(|e| {
                let msg = e.to_string();
                if msg.contains("unknown field") {
                    de::Error::custom(format!(
                        "{msg} (to compare with a table, write `{{ equals = {{ ... }} }}`)"
                    ))
                } else {
                    de::Error::custom(msg)
                }
            }),
            other => Ok(Self::equals(other)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Result<AssertSpec, String> {
        #[derive(Deserialize)]
        struct F {
            assert: AssertSpec,
        }
        toml::from_str::<F>(text)
            .map(|f| f.assert)
            .map_err(|e| e.message().to_owned())
    }

    #[test]
    fn shorthand_and_tables() {
        let a = parse(
            r#"
            [assert]
            status = [200, 201]
            duration_ms = { lt = 800 }
            body = { contains = "snap", not_contains = "error" }

            [assert.headers]
            Content-Type = { starts_with = "application/json" }
            x-count = 3

            [assert.json]
            "$.id" = { type = "integer", gt = 0 }
            "$.name" = "snap"
            "$.user" = { equals = { name = "x" } }
            "$.tags" = { length = 2, contains = "a" }
            "#,
        )
        .unwrap();
        assert_eq!(a.status, Some(Check::equals(serde_json::json!([200, 201]))));
        assert_eq!(a.duration_ms.unwrap().lt, Some(800.into()));
        let keys: Vec<&str> = a.json.0.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(keys, ["$.id", "$.name", "$.user", "$.tags"]);
        assert_eq!(a.json.0[0].1.type_, Some(JsonType::Integer));
        assert_eq!(a.json.0[1].1, Check::equals("snap"));
        assert_eq!(a.json.0[2].1.equals, Some(serde_json::json!({"name": "x"})));
        assert_eq!(a.headers.0[1].1, Check::equals(3));
    }

    #[test]
    fn typos_are_errors() {
        let err = parse("[assert]\nstatuss = 200").unwrap_err();
        assert!(err.contains("unknown field `statuss`"), "{err}");
        let err = parse("[assert.json]\n\"$.a\" = { equal = 1 }").unwrap_err();
        assert!(
            err.contains("unknown field `equal`") && err.contains("equals = {"),
            "{err}"
        );
        let err = parse("[assert.json]\n\"$.a\" = { type = \"text\" }").unwrap_err();
        assert!(err.contains("unknown variant `text`"), "{err}");
    }

    #[test]
    fn strings_are_mapped_for_interpolation() {
        let mut a = parse("[assert]\nschema = \"{{dir}}/s.json\"\n[assert.json]\n\"$.a\" = { one_of = [\"{{x}}\", 1] }\n\"$.b\" = { matches = \"^{{x}}$\" }").unwrap();
        let mut seen = Vec::new();
        a.try_map_strings(|place, s| {
            seen.push(place.to_owned());
            Ok::<_, ()>(s.replace("{{x}}", "X").replace("{{dir}}", "d"))
        })
        .unwrap();
        assert_eq!(a.schema, Some(PathBuf::from("d/s.json")));
        assert_eq!(
            a.json.0[0].1.one_of,
            Some(vec![serde_json::json!("X"), serde_json::json!(1)])
        );
        assert_eq!(a.json.0[1].1.matches.as_deref(), Some("^X$"));
        assert!(seen.contains(&"assert.json.$.a".to_owned()));
    }
}
