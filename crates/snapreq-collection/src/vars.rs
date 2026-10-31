//! Variables: `[vars]` tables, environments, and the scopes a request sees.
//!
//! Precedence, highest first: runtime → request → folders (innermost first) →
//! environment → collection → global. Collection variables are defaults; an environment
//! overrides them; folder and request variables are deliberate local overrides.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::de::{self, MapAccess, Visitor};
use serde::ser::{SerializeMap, SerializeStruct};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};
use crate::load::{Collection, ENVIRONMENTS_DIR};

/// One variable's definition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Variable {
    pub name: String,
    /// `None` for a secret whose value is kept outside the collection.
    pub value: Option<String>,
    /// Secret values are hidden in output and never written to collection files.
    pub secret: bool,
}

/// An ordered `name = value` table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Variables(pub Vec<Variable>);

impl Variables {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&Variable> {
        self.0.iter().find(|v| v.name == name)
    }

    /// Sets (or replaces) a plain variable.
    pub fn set(&mut self, name: impl Into<String>, value: impl Into<String>) {
        let name = name.into();
        let value = Some(value.into());
        match self.0.iter_mut().find(|v| v.name == name) {
            Some(v) => v.value = value,
            None => self.0.push(Variable {
                name,
                value,
                secret: false,
            }),
        }
    }
}

/// Variable names: letters, digits, `_`, `-` and `.`; not starting with `$`
/// (reserved for dynamic variables such as `{{$uuid}}`).
pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

impl<'de> Deserialize<'de> for Variables {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = Variables;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a table of variables")
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<Variables, A::Error> {
                let mut vars: Vec<Variable> = Vec::new();
                while let Some(name) = map.next_key::<String>()? {
                    if !valid_name(&name) {
                        return Err(de::Error::custom(format!(
                            "invalid variable name `{name}` (use letters, digits, `_`, `-` and `.`)"
                        )));
                    }
                    let VarValue { value, secret } = map.next_value()?;
                    vars.push(Variable {
                        name,
                        value,
                        secret,
                    });
                }
                Ok(Variables(vars))
            }
        }
        d.deserialize_map(V)
    }
}

/// A scalar (string, number, boolean) or `{ value = ..., secret = true }`.
struct VarValue {
    value: Option<String>,
    secret: bool,
}

impl<'de> Deserialize<'de> for VarValue {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = VarValue;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a string, number, boolean, or { value = \"...\", secret = true }")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<VarValue, E> {
                Ok(VarValue {
                    value: Some(v.to_owned()),
                    secret: false,
                })
            }
            fn visit_i64<E: de::Error>(self, v: i64) -> std::result::Result<VarValue, E> {
                self.visit_str(&v.to_string())
            }
            fn visit_u64<E: de::Error>(self, v: u64) -> std::result::Result<VarValue, E> {
                self.visit_str(&v.to_string())
            }
            fn visit_f64<E: de::Error>(self, v: f64) -> std::result::Result<VarValue, E> {
                self.visit_str(&v.to_string())
            }
            fn visit_bool<E: de::Error>(self, v: bool) -> std::result::Result<VarValue, E> {
                self.visit_str(&v.to_string())
            }
            fn visit_map<A: MapAccess<'de>>(
                self,
                mut map: A,
            ) -> std::result::Result<VarValue, A::Error> {
                let mut out = VarValue {
                    value: None,
                    secret: false,
                };
                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "value" => out.value = Some(map.next_value::<String>()?),
                        "secret" => out.secret = map.next_value()?,
                        other => return Err(de::Error::unknown_field(other, &["value", "secret"])),
                    }
                }
                if out.value.is_none() && !out.secret {
                    return Err(de::Error::missing_field("value"));
                }
                Ok(out)
            }
        }
        d.deserialize_any(V)
    }
}

impl Serialize for Variables {
    fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        struct One<'a>(&'a Variable);
        impl Serialize for One<'_> {
            fn serialize<S: Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
                match (&self.0.value, self.0.secret) {
                    (Some(v), false) => s.serialize_str(v),
                    // Secret values are never written; only the marker is.
                    (_, true) => {
                        let mut st = s.serialize_struct("Secret", 1)?;
                        st.serialize_field("secret", &true)?;
                        st.end()
                    }
                    (None, false) => s.serialize_str(""),
                }
            }
        }
        let mut map = s.serialize_map(Some(self.0.len()))?;
        for v in &self.0 {
            map.serialize_entry(&v.name, &One(v))?;
        }
        map.end()
    }
}

/// An environment file: `environments/<name>.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub name: String,
    pub path: PathBuf,
    pub vars: Variables,
}

impl Environment {
    /// Loads `environments/<name>.toml` of a collection.
    pub fn load(collection: &Collection, name: &str) -> Result<Self> {
        let path = collection
            .root
            .join(ENVIRONMENTS_DIR)
            .join(format!("{name}.toml"));
        if !path.is_file() {
            let known = Self::list(collection).unwrap_or_default();
            let hint = if known.is_empty() {
                "the collection has no environments".to_owned()
            } else {
                format!("available: {}", known.join(", "))
            };
            return Err(Error::invalid(
                &path,
                format!("no environment `{name}` ({hint})"),
            ));
        }
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let vars: Variables = toml::from_str(&text).map_err(|e| Error::toml(&path, &text, &e))?;
        Ok(Self {
            name: name.to_owned(),
            path,
            vars,
        })
    }

    /// Names of the collection's environments, sorted.
    pub fn list(collection: &Collection) -> Result<Vec<String>> {
        let dir = collection.root.join(ENVIRONMENTS_DIR);
        if !dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .map_err(|e| Error::io(&dir, e))?
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                (!name.starts_with('.'))
                    .then(|| name.strip_suffix(".toml").map(str::to_owned))
                    .flatten()
            })
            .collect();
        names.sort();
        Ok(names)
    }

    pub fn file_path(&self) -> &Path {
        &self.path
    }
}

/// Where a variable's value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    Runtime,
    Request,
    /// Folder key path, e.g. `users/admin`.
    Folder(String),
    /// Environment name.
    Environment(String),
    Collection,
    Global,
}

impl fmt::Display for Scope {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Runtime => f.write_str("runtime"),
            Self::Request => f.write_str("request"),
            Self::Folder(k) => write!(f, "folder `{k}`"),
            Self::Environment(n) => write!(f, "environment `{n}`"),
            Self::Collection => f.write_str("collection"),
            Self::Global => f.write_str("global"),
        }
    }
}

/// The variables visible to one request, layered by precedence.
#[derive(Debug, Clone, Default)]
pub struct Scopes {
    /// Highest precedence first.
    layers: Vec<(Scope, Variables)>,
}

impl Scopes {
    /// Builds the layers for the request at `key_path`.
    pub fn for_request(
        collection: &Collection,
        key_path: &str,
        environment: Option<&Environment>,
        globals: &Variables,
        runtime: &Variables,
    ) -> Self {
        let mut layers = vec![(Scope::Runtime, runtime.clone())];
        if let Some(request) = collection.find(key_path) {
            layers.push((Scope::Request, request.file.vars.clone()));
        }
        let folders = collection.folders_of(key_path);
        let mut prefix = String::new();
        let mut folder_layers = Vec::new();
        for f in folders {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(&f.key);
            folder_layers.push((Scope::Folder(prefix.clone()), f.file.vars.clone()));
        }
        // Innermost folder first.
        layers.extend(folder_layers.into_iter().rev());
        if let Some(env) = environment {
            layers.push((Scope::Environment(env.name.clone()), env.vars.clone()));
        }
        layers.push((Scope::Collection, collection.file.vars.clone()));
        layers.push((Scope::Global, globals.clone()));
        Self { layers }
    }

    /// The winning definition of `name` and the scope it came from.
    pub fn lookup(&self, name: &str) -> Option<(&Variable, &Scope)> {
        self.layers
            .iter()
            .find_map(|(scope, vars)| vars.get(name).map(|v| (v, scope)))
    }

    /// Every visible variable with its winning definition, in first-seen order
    /// (for listing in the CLI and UI).
    pub fn visible(&self) -> Vec<(&Variable, &Scope)> {
        let mut out: Vec<(&Variable, &Scope)> = Vec::new();
        for (scope, vars) in &self.layers {
            for v in &vars.0 {
                if !out.iter().any(|(seen, _)| seen.name == v.name) {
                    out.push((v, scope));
                }
            }
        }
        out
    }

    #[cfg(test)]
    pub(crate) fn push_layer_for_tests(&mut self, vars: Variables) {
        self.layers.push((Scope::Runtime, vars));
    }

    /// The scopes searched, highest precedence first (for error messages).
    pub fn scope_names(&self) -> Vec<String> {
        self.layers.iter().map(|(s, _)| s.to_string()).collect()
    }

    /// Adds or replaces a runtime variable (highest precedence).
    pub fn set_runtime(&mut self, name: impl Into<String>, value: impl Into<String>) {
        if let Some((_, vars)) = self.layers.iter_mut().find(|(s, _)| *s == Scope::Runtime) {
            vars.set(name, value);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize, Serialize)]
    struct Doc {
        vars: Variables,
    }

    fn parse(text: &str) -> std::result::Result<Variables, String> {
        toml::from_str::<Doc>(text)
            .map(|d| d.vars)
            .map_err(|e| e.message().to_owned())
    }

    #[test]
    fn value_forms() {
        let v = parse(
            "[vars]\nbase_url = \"http://a\"\nport = 8080\ntoken = { secret = true }\napi.key = { value = \"k\", secret = true }\n",
        );
        // Dotted keys are TOML tables, not names with dots; quote them instead.
        assert!(v.is_err());

        let v = parse(
            "[vars]\nbase_url = \"http://a\"\nport = 8080\ntoken = { secret = true }\n\"api.key\" = { value = \"k\", secret = true }\n",
        )
        .unwrap();
        assert_eq!(v.get("port").unwrap().value.as_deref(), Some("8080"));
        let token = v.get("token").unwrap();
        assert!(token.secret && token.value.is_none());
        assert!(v.get("api.key").unwrap().secret);
    }

    #[test]
    fn invalid_definitions() {
        assert!(
            parse("[vars]\n\"bad name\" = \"x\"\n")
                .unwrap_err()
                .contains("invalid variable name")
        );
        assert!(
            parse("[vars]\n\"$uuid\" = \"x\"\n")
                .unwrap_err()
                .contains("invalid variable name")
        );
        assert!(
            parse("[vars]\nx = { secrt = true }\n")
                .unwrap_err()
                .contains("unknown field `secrt`")
        );
        assert!(
            parse("[vars]\nx = {}\n")
                .unwrap_err()
                .contains("missing field `value`")
        );
    }

    #[test]
    fn secret_values_are_never_serialized() {
        let mut vars = Variables::default();
        vars.set("plain", "1");
        vars.0.push(Variable {
            name: "token".into(),
            value: Some("do-not-write".into()),
            secret: true,
        });
        let text = crate::save::to_toml(&Doc { vars }).unwrap();
        assert_eq!(text, "[vars]\nplain = \"1\"\ntoken = { secret = true }\n");
    }
}
