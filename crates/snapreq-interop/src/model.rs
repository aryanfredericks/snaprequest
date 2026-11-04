//! What an import produces, and writing it as collection files.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use snapreq_collection::load::{COLLECTION_FILE, ENVIRONMENTS_DIR, FOLDER_FILE};
use snapreq_collection::save::{
    save_collection, save_environment, save_flow, save_folder, save_request, to_toml,
};
use snapreq_collection::sealed::SecretKey;
use snapreq_collection::{
    Collection, CollectionFile, FLOW_SUFFIX, FlowFile, FolderFile, RequestFile, Variable,
    Variables, secrets,
};

use crate::error::{Error, Result};
use crate::util::{placeholders, slug};

/// Built-in dynamic variables (`docs/FORMAT.md`).
const DYNAMIC: [&str; 4] = ["$uuid", "$timestamp", "$isoTimestamp", "$randomInt"];

/// An imported collection, ready to be written.
#[derive(Debug, Clone)]
pub struct Imported {
    pub collection: CollectionFile,
    /// Top-level requests, folders and flows, in source order.
    pub items: Vec<Node>,
    /// Environments by name. Secret variables may carry values: they go to `.secrets`.
    pub environments: Vec<(String, Variables)>,
    /// Things that could not be imported exactly.
    pub warnings: Vec<String>,
    /// Variable names the source uses: credentials moved to secrets must not take them.
    reserved: HashSet<String>,
}

/// One item of an imported collection.
#[derive(Debug, Clone)]
pub enum Node {
    /// A request; its file name comes from its `name`.
    Request(RequestFile),
    /// A folder with its own settings and children.
    Folder {
        name: String,
        file: FolderFile,
        items: Vec<Node>,
    },
    /// A flow (`*.flow.toml`); its file name comes from `name`.
    Flow { name: String, file: FlowFile },
}

/// What [`Imported::write`] wrote.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct Summary {
    pub name: String,
    pub requests: usize,
    pub folders: usize,
    pub flows: usize,
    pub environments: Vec<String>,
    /// Secret values stored in `.secrets` (never in collection files).
    pub secrets: usize,
    pub warnings: Vec<String>,
}

impl Imported {
    pub(crate) fn new(name: impl Into<String>) -> Self {
        Self {
            collection: CollectionFile::new(name),
            items: Vec::new(),
            environments: Vec::new(),
            warnings: Vec::new(),
            reserved: HashSet::new(),
        }
    }

    /// Reserves the placeholder names used in the source text (`{{ _.name }}` counts as `name`).
    pub(crate) fn reserve(&mut self, source: &str) {
        for name in placeholders(source) {
            self.reserved
                .insert(name.trim_start_matches("_.").to_owned());
            self.reserved.insert(name);
        }
    }

    fn taken(&self, name: &str) -> bool {
        self.reserved.contains(name)
            || self.collection.vars.get(name).is_some()
            || self.environments.iter().any(|(_, v)| v.get(name).is_some())
    }

    pub(crate) fn warn(&mut self, message: impl Into<String>) {
        let message = message.into();
        if !self.warnings.contains(&message) {
            self.warnings.push(message);
        }
    }

    /// Moves a credential into a secret collection variable and returns the placeholder to use
    /// instead. Values that already are (or contain) placeholders stay as they are; the same
    /// value reuses its variable.
    pub(crate) fn secret(&mut self, hint: &str, value: &str) -> String {
        if value.is_empty() || value.contains("{{") {
            return value.to_owned();
        }
        if let Some(v) = self
            .collection
            .vars
            .0
            .iter()
            .find(|v| v.secret && v.value.as_deref() == Some(value))
        {
            return format!("{{{{{}}}}}", v.name);
        }
        let base = crate::util::var_name(hint);
        let mut name = base.clone();
        let mut n = 2;
        while self.taken(&name) {
            name = format!("{base}_{n}");
            n += 1;
        }
        self.collection.vars.0.push(Variable {
            name: name.clone(),
            value: Some(value.to_owned()),
            secret: true,
        });
        format!("{{{{{name}}}}}")
    }

    /// Declares a secret variable without a value (the user sets it later), unless it exists.
    pub(crate) fn declare_secret(&mut self, name: &str) {
        if self.collection.vars.get(name).is_none() {
            self.collection.vars.0.push(Variable {
                name: name.to_owned(),
                value: None,
                secret: true,
            });
        }
    }

    /// Adds an environment, renaming it if the name is taken; returns the name used.
    pub(crate) fn add_environment(&mut self, name: &str, vars: Variables) -> String {
        let base = slug(name, "environment");
        let mut unique = base.clone();
        let mut n = 2;
        while self.environments.iter().any(|(e, _)| *e == unique) {
            unique = format!("{base}-{n}");
            n += 1;
        }
        self.environments.push((unique.clone(), vars));
        unique
    }

    /// Final checks shared by every importer: warns about variables nothing defines.
    pub(crate) fn finish(&mut self) {
        let mut defined: HashSet<String> = HashSet::new();
        let mut used: BTreeSet<String> = BTreeSet::new();
        let add_vars = |vars: &Variables, defined: &mut HashSet<String>| {
            for v in &vars.0 {
                defined.insert(v.name.clone());
            }
        };
        add_vars(&self.collection.vars, &mut defined);
        for (_, vars) in &self.environments {
            add_vars(vars, &mut defined);
        }
        collect(&self.items, &mut defined, &mut used);
        if let Ok(text) = to_toml(&self.collection) {
            used.extend(placeholders(&text));
        }
        let missing: Vec<String> = used
            .iter()
            .filter(|u| !defined.contains(*u) && !u.starts_with('$'))
            .cloned()
            .collect();
        self.warnings.retain(|w| {
            !w.starts_with("variables used but not defined") && !w.starts_with("dynamic variables")
        });
        let dynamic: Vec<&String> = used
            .iter()
            .filter(|u| u.starts_with('$') && !DYNAMIC.contains(&u.as_str()))
            .collect();
        if !dynamic.is_empty() {
            self.warnings.push(format!(
                "dynamic variables SnapRequest does not have: {} (supported: {})",
                dynamic
                    .iter()
                    .map(|d| format!("{{{{{d}}}}}"))
                    .collect::<Vec<_>>()
                    .join(", "),
                DYNAMIC.join(", ")
            ));
        }
        if !missing.is_empty() {
            self.warnings.push(format!(
                "variables used but not defined anywhere: {} (set them in an environment or with --var)",
                missing.join(", ")
            ));
        }
    }

    /// Counts what [`Imported::write`] would write.
    pub fn summary(&self) -> Summary {
        fn count(items: &[Node], s: &mut Summary) {
            for item in items {
                match item {
                    Node::Request(_) => s.requests += 1,
                    Node::Folder { items, .. } => {
                        s.folders += 1;
                        count(items, s);
                    }
                    Node::Flow { .. } => s.flows += 1,
                }
            }
        }
        let mut s = Summary {
            name: self.collection.name.clone(),
            environments: self.environments.iter().map(|(n, _)| n.clone()).collect(),
            secrets: self.secret_values().len(),
            warnings: self.warnings.clone(),
            ..Summary::default()
        };
        count(&self.items, &mut s);
        s
    }

    /// `(environment, name, value)` of every secret value to store.
    fn secret_values(&self) -> Vec<(Option<&str>, &str, &str)> {
        let mut out = Vec::new();
        for v in &self.collection.vars.0 {
            if let (true, Some(value)) = (v.secret, &v.value) {
                out.push((None, v.name.as_str(), value.as_str()));
            }
        }
        for (env, vars) in &self.environments {
            for v in &vars.0 {
                if let (true, Some(value)) = (v.secret, &v.value) {
                    out.push((Some(env.as_str()), v.name.as_str(), value.as_str()));
                }
            }
        }
        out
    }

    /// Writes the collection into `dir`, which must be missing or empty. Secret values go to
    /// `.secrets/` (sealed with `key`). The result is loaded back to make sure it is valid.
    pub fn write(&self, dir: &Path, key: Option<&SecretKey>) -> Result<Summary> {
        if std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some()) {
            return Err(Error::TargetExists(dir.to_owned()));
        }
        let result = self.write_files(dir, key);
        if result.is_err() {
            // Leave nothing half-written behind.
            let _ = std::fs::remove_dir_all(dir);
        }
        result
    }

    fn write_files(&self, dir: &Path, key: Option<&SecretKey>) -> Result<Summary> {
        let mut file = self.collection.clone();
        let mut keys = Keys::root();
        file.order = write_items(dir, &self.items, &mut keys)?;
        save_collection(&dir.join(COLLECTION_FILE), &file)?;
        for (name, vars) in &self.environments {
            save_environment(
                &dir.join(ENVIRONMENTS_DIR).join(format!("{name}.toml")),
                vars,
            )?;
        }
        for (env, name, value) in self.secret_values() {
            secrets::store(dir, env, name, value, key)?;
        }
        Collection::load(dir)?;
        Ok(self.summary())
    }
}

/// Every placeholder used and every variable defined below `items`.
fn collect(items: &[Node], defined: &mut HashSet<String>, used: &mut BTreeSet<String>) {
    for item in items {
        let text = match item {
            Node::Request(r) => {
                defined.extend(r.vars.0.iter().map(|v| v.name.clone()));
                defined.extend(r.capture.0.iter().map(|(n, _)| n.clone()));
                to_toml(r)
            }
            Node::Folder { file, items, .. } => {
                defined.extend(file.vars.0.iter().map(|v| v.name.clone()));
                collect(items, defined, used);
                to_toml(file)
            }
            Node::Flow { file, .. } => {
                defined.extend(file.vars.0.iter().map(|v| v.name.clone()));
                for s in &file.steps {
                    defined.extend(s.vars.0.iter().map(|v| v.name.clone()));
                    defined.extend(s.capture.0.iter().map(|(n, _)| n.clone()));
                }
                to_toml(file)
            }
        };
        if let Ok(text) = text {
            used.extend(placeholders(&text));
        }
    }
}

/// File names in one directory: unique, case-insensitively, and not reserved.
struct Keys(HashSet<String>);

impl Keys {
    fn root() -> Self {
        Self(
            ["snapreq", "folder", ENVIRONMENTS_DIR]
                .map(str::to_owned)
                .into(),
        )
    }

    fn folder() -> Self {
        Self(["folder".to_owned()].into())
    }

    fn take(&mut self, name: &str, fallback: &str) -> String {
        let base = slug(name, fallback);
        let mut key = base.clone();
        let mut n = 2;
        while self.0.contains(&key) {
            key = format!("{base}-{n}");
            n += 1;
        }
        self.0.insert(key.clone());
        key
    }
}

/// Writes `items` into `dir`; returns the keys of requests and folders in order.
fn write_items(dir: &Path, items: &[Node], keys: &mut Keys) -> Result<Vec<String>> {
    let mut order = Vec::new();
    for item in items {
        match item {
            Node::Request(r) => {
                let key = keys.take(r.name.as_deref().unwrap_or("request"), "request");
                save_request(&dir.join(format!("{key}.toml")), r)?;
                order.push(key);
            }
            Node::Folder { name, file, items } => {
                let key = keys.take(name, "folder");
                let sub: PathBuf = dir.join(&key);
                let mut file = file.clone();
                file.name = Some(name.clone());
                file.order = write_items(&sub, items, &mut Keys::folder())?;
                save_folder(&sub.join(FOLDER_FILE), &file)?;
                order.push(key);
            }
            Node::Flow { name, file } => {
                let key = keys.take(name, "flow");
                save_flow(&dir.join(format!("{key}{FLOW_SUFFIX}")), file)?;
            }
        }
    }
    Ok(order)
}
