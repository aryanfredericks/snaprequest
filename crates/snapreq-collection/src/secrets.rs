//! Values for secret variables, kept outside the committed collection.
//!
//! A variable declared `{ secret = true }` gets its value from, in order:
//! 1. the runtime scope (`--var`), which already wins in [`Scopes`];
//! 2. the process environment: `SNAPREQ_SECRET_<NAME>` (upper case, `.` and `-` → `_`);
//! 3. `<collection>/.secrets/<environment>.toml`;
//! 4. `<collection>/.secrets/shared.toml`.
//!
//! The `.secrets` directory contains a `.gitignore` with `*`, written by SnapRequest, so the
//! files stay out of git even when the project's own `.gitignore` does not mention them.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::load::Collection;
use crate::vars::{Scopes, Variable, Variables};

/// Directory (inside the collection) holding local secret values.
pub const SECRETS_DIR: &str = ".secrets";
/// File in [`SECRETS_DIR`] used for every environment.
pub const SHARED_FILE: &str = "shared.toml";

/// Where a secret value came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretSource {
    /// `SNAPREQ_SECRET_<NAME>`.
    EnvVar(String),
    File(PathBuf),
}

impl std::fmt::Display for SecretSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EnvVar(v) => write!(f, "environment variable {v}"),
            Self::File(p) => write!(f, "{}", p.display()),
        }
    }
}

/// Secret values available for one collection and environment.
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    /// Highest priority first.
    files: Vec<(PathBuf, Variables)>,
    /// Snapshot of `SNAPREQ_SECRET_*` variables (name → value).
    env: Vec<(String, String)>,
}

/// `SNAPREQ_SECRET_<NAME>` for a variable name.
pub fn env_var_name(name: &str) -> String {
    let upper: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_uppercase()
            } else {
                '_'
            }
        })
        .collect();
    format!("SNAPREQ_SECRET_{upper}")
}

impl Secrets {
    /// Loads the secret files of `collection` for `environment`, and takes the
    /// `SNAPREQ_SECRET_*` variables from `env_vars` (normally `std::env::vars()`).
    pub fn load(
        collection: &Collection,
        environment: Option<&str>,
        env_vars: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self> {
        let dir = collection.root.join(SECRETS_DIR);
        let mut files = Vec::new();
        let mut candidates = Vec::new();
        if let Some(env) = environment {
            candidates.push(dir.join(format!("{env}.toml")));
        }
        candidates.push(dir.join(SHARED_FILE));
        for path in candidates {
            if path.is_file() {
                let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
                let vars: Variables =
                    toml::from_str(&text).map_err(|e| Error::toml(&path, &text, &e))?;
                files.push((path, vars));
            }
        }
        let env = env_vars
            .into_iter()
            .filter(|(k, _)| k.starts_with("SNAPREQ_SECRET_"))
            .collect();
        Ok(Self { files, env })
    }

    /// The value of secret `name` and where it came from.
    pub fn get(&self, name: &str) -> Option<(String, SecretSource)> {
        let key = env_var_name(name);
        if let Some((_, v)) = self.env.iter().find(|(k, _)| *k == key) {
            return Some((v.clone(), SecretSource::EnvVar(key)));
        }
        self.files.iter().find_map(|(path, vars)| {
            vars.get(name)
                .and_then(|v| v.value.clone())
                .map(|v| (v, SecretSource::File(path.clone())))
        })
    }

    /// Fills every secret variable in `scopes` that has no value yet.
    pub fn fill(&self, scopes: &mut Scopes) {
        scopes.for_each_variable_mut(|var: &mut Variable| {
            if var.secret && var.value.is_none() {
                var.value = self.get(&var.name).map(|(v, _)| v);
            }
        });
    }
}

/// Stores a secret value in `.secrets/<environment>.toml` (or `shared.toml` when
/// `environment` is `None`), creating the directory and its `.gitignore`.
pub fn store(
    collection_root: &Path,
    environment: Option<&str>,
    name: &str,
    value: &str,
) -> Result<PathBuf> {
    if !crate::vars::valid_name(name) {
        return Err(Error::invalid(
            collection_root,
            format!("invalid variable name `{name}`"),
        ));
    }
    let dir = collection_root.join(SECRETS_DIR);
    std::fs::create_dir_all(&dir).map_err(|e| Error::io(&dir, e))?;
    let ignore = dir.join(".gitignore");
    if !ignore.is_file() {
        std::fs::write(
            &ignore,
            "# Written by SnapRequest: never commit secret values.\n*\n",
        )
        .map_err(|e| Error::io(&ignore, e))?;
    }
    let path = match environment {
        Some(env) => dir.join(format!("{env}.toml")),
        None => dir.join(SHARED_FILE),
    };
    let mut vars: Variables = if path.is_file() {
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        toml::from_str(&text).map_err(|e| Error::toml(&path, &text, &e))?
    } else {
        Variables::default()
    };
    // Stored as plain values: this file *is* the secret store.
    vars.set(name, value);
    let mut text = String::from("# Secret values for SnapRequest. Do not commit this file.\n");
    let mut doc = toml_edit::DocumentMut::new();
    for v in &vars.0 {
        doc[v.name.as_str()] = toml_edit::value(v.value.clone().unwrap_or_default());
    }
    text.push_str(&doc.to_string());
    std::fs::write(&path, text).map_err(|e| Error::io(&path, e))?;
    restrict_permissions(&path)?;
    Ok(path)
}

/// Makes a secrets file readable by its owner only (Unix), like SSH keys.
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| Error::io(path, e))
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_var_names() {
        assert_eq!(env_var_name("token"), "SNAPREQ_SECRET_TOKEN");
        assert_eq!(env_var_name("api.key-2"), "SNAPREQ_SECRET_API_KEY_2");
    }
}
