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
//!
//! Values in the files may be sealed (`enc:v1:…`, see [`crate::sealed`]); the server always
//! seals what it writes. Sealed values open with the secret key; without it they count as unset
//! and their names are listed by [`Secrets::locked`].

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};
use crate::load::Collection;
use crate::sealed::SecretKey;
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
    /// Sealed values that could not be opened (no key, or another key).
    locked: Vec<String>,
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
    /// `SNAPREQ_SECRET_*` variables from `env_vars` (normally `std::env::vars()`). Sealed values
    /// open with the key in `SNAPREQ_SECRET_KEY`, if `env_vars` has it.
    pub fn load(
        collection: &Collection,
        environment: Option<&str>,
        env_vars: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self> {
        let env_vars: Vec<(String, String)> = env_vars.into_iter().collect();
        let key = env_vars
            .iter()
            .find(|(k, _)| k == crate::sealed::KEY_ENV)
            .and_then(|(_, v)| SecretKey::decode(v).ok());
        Self::load_with_key(collection, environment, env_vars, key.as_ref())
    }

    /// [`Secrets::load`] with an explicit key for sealed values.
    pub fn load_with_key(
        collection: &Collection,
        environment: Option<&str>,
        env_vars: impl IntoIterator<Item = (String, String)>,
        key: Option<&SecretKey>,
    ) -> Result<Self> {
        let dir = collection.root.join(SECRETS_DIR);
        let mut files = Vec::new();
        let mut locked = Vec::new();
        let mut candidates = Vec::new();
        if let Some(env) = environment {
            candidates.push(dir.join(format!("{env}.toml")));
        }
        candidates.push(dir.join(SHARED_FILE));
        for path in candidates {
            if path.is_file() {
                let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
                let mut vars: Variables =
                    toml::from_str(&text).map_err(|e| Error::toml(&path, &text, &e))?;
                for v in &mut vars.0 {
                    let Some(stored) = v.value.as_deref().filter(|s| crate::sealed::is_sealed(s))
                    else {
                        continue;
                    };
                    v.value = key.and_then(|k| k.open(&v.name, stored).ok());
                    if v.value.is_none() && !locked.contains(&v.name) {
                        locked.push(v.name.clone());
                    }
                }
                files.push((path, vars));
            }
        }
        let env = env_vars
            .into_iter()
            .filter(|(k, _)| k.starts_with("SNAPREQ_SECRET_") && k != crate::sealed::KEY_ENV)
            .collect();
        Ok(Self { files, env, locked })
    }

    /// Names of sealed values that could not be opened (they count as unset).
    pub fn locked(&self) -> &[String] {
        &self.locked
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
/// `environment` is `None`), creating the directory and its `.gitignore`. With a `key` the value
/// is sealed; other values in the file are kept as they are.
pub fn store(
    collection_root: &Path,
    environment: Option<&str>,
    name: &str,
    value: &str,
    key: Option<&SecretKey>,
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
    // This file *is* the secret store: plain or sealed values, not declarations.
    let value = match key {
        Some(k) => k
            .seal(name, value)
            .map_err(|e| Error::invalid(collection_root, e))?,
        None => value.to_owned(),
    };
    vars.set(name, &value);
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

/// Seals every plain value in the `.secrets` files of the collection at `collection_root`;
/// returns how many were sealed.
pub fn seal_all(collection_root: &Path, key: &SecretKey) -> Result<usize> {
    let dir = collection_root.join(SECRETS_DIR);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(0);
    };
    let mut sealed = 0;
    for entry in entries.filter_map(std::result::Result::ok) {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "toml") {
            continue;
        }
        let text = std::fs::read_to_string(&path).map_err(|e| Error::io(&path, e))?;
        let vars: Variables = toml::from_str(&text).map_err(|e| Error::toml(&path, &text, &e))?;
        let plain: Vec<(String, String)> = vars
            .0
            .iter()
            .filter_map(|v| Some((v.name.clone(), v.value.clone()?)))
            .filter(|(_, v)| !crate::sealed::is_sealed(v))
            .collect();
        if plain.is_empty() {
            continue;
        }
        let env = path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| *s != "shared")
            .map(str::to_owned);
        for (name, value) in plain {
            store(collection_root, env.as_deref(), &name, &value, Some(key))?;
            sealed += 1;
        }
    }
    Ok(sealed)
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
