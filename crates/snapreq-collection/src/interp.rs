//! `{{variable}}` interpolation.
//!
//! - `{{name}}` (spaces inside the braces are allowed) takes the value from the scopes.
//! - Values may themselves contain placeholders; they are resolved recursively, and
//!   cycles are reported.
//! - `{{$uuid}}`, `{{$timestamp}}`, `{{$isoTimestamp}}` and `{{$randomInt}}` are generated
//!   anew for every placeholder.
//! - `\{{` is a literal `{{`.

use std::time::{SystemTime, UNIX_EPOCH};

use time::format_description::well_known::Rfc3339;

use crate::vars::Scopes;

/// Recursion limit for placeholders inside variable values.
const MAX_DEPTH: usize = 16;

/// Why a placeholder could not be resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterpError {
    Undefined { name: String, searched: Vec<String> },
    SecretWithoutValue { name: String },
    UnknownDynamic { name: String },
    Cycle { chain: Vec<String> },
    Unclosed,
    EmptyName,
}

impl std::fmt::Display for InterpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Undefined { name, searched } => write!(
                f,
                "variable `{name}` is not defined (searched: {})",
                searched.join(", ")
            ),
            Self::SecretWithoutValue { name } => write!(
                f,
                "secret variable `{name}` has no value; provide it with `--var {name}=...` or a secrets file"
            ),
            Self::UnknownDynamic { name } => write!(
                f,
                "unknown dynamic variable `{{{{${name}}}}}` (available: $uuid, $timestamp, $isoTimestamp, $randomInt)"
            ),
            Self::Cycle { chain } => {
                write!(f, "variables refer to each other: {}", chain.join(" → "))
            }
            Self::Unclosed => {
                f.write_str("`{{` without a closing `}}` (write `\\{{` for a literal `{{`)")
            }
            Self::EmptyName => f.write_str("empty placeholder `{{}}`"),
        }
    }
}

/// Resolves placeholders against a set of scopes and remembers the secret values it used.
#[derive(Debug)]
pub struct Interpolator<'a> {
    scopes: &'a Scopes,
    secrets: Vec<String>,
}

impl<'a> Interpolator<'a> {
    pub fn new(scopes: &'a Scopes) -> Self {
        Self {
            scopes,
            secrets: Vec::new(),
        }
    }

    /// Values of secret variables used so far (to hide them in traces and output).
    pub fn secrets_used(&self) -> &[String] {
        &self.secrets
    }

    pub fn into_secrets(self) -> Vec<String> {
        self.secrets
    }

    /// Replaces every placeholder in `text`.
    pub fn interpolate(&mut self, text: &str) -> Result<String, InterpError> {
        let mut stack = Vec::new();
        self.expand(text, &mut stack)
    }

    fn expand(&mut self, text: &str, stack: &mut Vec<String>) -> Result<String, InterpError> {
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(pos) = rest.find("{{") {
            // `\{{` is a literal `{{`.
            if rest[..pos].ends_with('\\') {
                out.push_str(&rest[..pos - 1]);
                out.push_str("{{");
                rest = &rest[pos + 2..];
                continue;
            }
            out.push_str(&rest[..pos]);
            let after = &rest[pos + 2..];
            let end = after.find("}}").ok_or(InterpError::Unclosed)?;
            let name = after[..end].trim();
            rest = &after[end + 2..];

            if name.is_empty() {
                return Err(InterpError::EmptyName);
            }
            if let Some(dynamic) = name.strip_prefix('$') {
                out.push_str(&dynamic_value(dynamic)?);
                continue;
            }
            out.push_str(&self.resolve(name, stack)?);
        }
        out.push_str(rest);
        Ok(out)
    }

    fn resolve(&mut self, name: &str, stack: &mut Vec<String>) -> Result<String, InterpError> {
        if stack.iter().any(|n| n == name) || stack.len() >= MAX_DEPTH {
            let mut chain = stack.clone();
            chain.push(name.to_owned());
            return Err(InterpError::Cycle { chain });
        }
        let Some((var, _)) = self.scopes.lookup(name) else {
            return Err(InterpError::Undefined {
                name: name.to_owned(),
                searched: self.scopes.scope_names(),
            });
        };
        let Some(raw) = var.value.clone() else {
            return Err(InterpError::SecretWithoutValue {
                name: name.to_owned(),
            });
        };
        let secret = var.secret;
        stack.push(name.to_owned());
        let value = self.expand(&raw, stack)?;
        stack.pop();
        if secret && !value.is_empty() && !self.secrets.contains(&value) {
            self.secrets.push(value.clone());
        }
        Ok(value)
    }
}

fn dynamic_value(name: &str) -> Result<String, InterpError> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    Ok(match name {
        "uuid" => uuid::Uuid::new_v4().to_string(),
        "timestamp" => now.as_secs().to_string(),
        "isoTimestamp" => time::OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .unwrap_or_default(),
        "randomInt" => (uuid::Uuid::new_v4().as_u128() % 1001).to_string(),
        other => {
            return Err(InterpError::UnknownDynamic {
                name: other.to_owned(),
            });
        }
    })
}

/// Whether `text` contains any placeholder.
pub fn has_placeholders(text: &str) -> bool {
    text.contains("{{")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vars::{Variable, Variables};

    fn scopes(vars: &[(&str, &str, bool)]) -> Scopes {
        let mut runtime = Variables::default();
        for (name, value, secret) in vars {
            runtime.0.push(Variable {
                name: (*name).into(),
                value: Some((*value).into()),
                secret: *secret,
            });
        }
        runtime.0.push(Variable {
            name: "no_value".into(),
            value: None,
            secret: true,
        });
        let mut s = Scopes::default();
        s.push_layer_for_tests(runtime);
        s
    }

    #[test]
    fn plain_nested_and_spaced_placeholders() {
        let s = scopes(&[
            ("host", "api.test", false),
            ("base", "https://{{host}}", false),
            ("v", "2", false),
        ]);
        let mut i = Interpolator::new(&s);
        assert_eq!(
            i.interpolate("{{base}}/v{{ v }}/x").unwrap(),
            "https://api.test/v2/x"
        );
        assert_eq!(i.interpolate("no placeholders").unwrap(), "no placeholders");
        assert_eq!(
            i.interpolate(r"literal \{{v}} and {{v}}").unwrap(),
            "literal {{v}} and 2"
        );
    }

    #[test]
    fn errors() {
        let s = scopes(&[("a", "{{b}}", false), ("b", "{{a}}", false)]);
        let mut i = Interpolator::new(&s);
        assert_eq!(
            i.interpolate("{{a}}").unwrap_err(),
            InterpError::Cycle {
                chain: vec!["a".into(), "b".into(), "a".into()]
            }
        );
        let err = i.interpolate("{{missing}}").unwrap_err();
        assert_eq!(
            err.to_string(),
            "variable `missing` is not defined (searched: runtime)"
        );
        assert!(matches!(
            i.interpolate("{{no_value}}").unwrap_err(),
            InterpError::SecretWithoutValue { .. }
        ));
        assert_eq!(i.interpolate("{{oops").unwrap_err(), InterpError::Unclosed);
        assert_eq!(i.interpolate("{{ }}").unwrap_err(), InterpError::EmptyName);
        assert!(
            i.interpolate("{{$nope}}")
                .unwrap_err()
                .to_string()
                .contains("$uuid")
        );
    }

    #[test]
    fn dynamic_variables() {
        let s = scopes(&[]);
        let mut i = Interpolator::new(&s);
        let a = i.interpolate("{{$uuid}}").unwrap();
        let b = i.interpolate("{{$uuid}}").unwrap();
        assert_eq!(a.len(), 36);
        assert_ne!(a, b);
        assert!(
            i.interpolate("{{$timestamp}}")
                .unwrap()
                .parse::<u64>()
                .unwrap()
                > 1_700_000_000
        );
        assert!(i.interpolate("{{$isoTimestamp}}").unwrap().ends_with('Z'));
        assert!(
            i.interpolate("{{$randomInt}}")
                .unwrap()
                .parse::<u32>()
                .unwrap()
                <= 1000
        );
    }

    #[test]
    fn secret_values_used_are_collected() {
        let s = scopes(&[
            ("token", "t0p-s3cret", true),
            ("auth", "Bearer {{token}}", false),
        ]);
        let mut i = Interpolator::new(&s);
        assert_eq!(i.interpolate("{{auth}}").unwrap(), "Bearer t0p-s3cret");
        i.interpolate("{{token}}").unwrap();
        assert_eq!(i.secrets_used(), ["t0p-s3cret"]);
    }
}
