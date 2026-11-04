//! Reading a collection directory.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;

use crate::error::{Error, Result, Warning};
use crate::files::{CollectionFile, FORMAT_VERSION, FolderFile, RequestFile};
use crate::flow::{FLOW_SUFFIX, FlowFile};

/// The collection's root file name.
pub const COLLECTION_FILE: &str = "snapreq.toml";
/// A folder's optional settings file name.
pub const FOLDER_FILE: &str = "folder.toml";
/// The top-level directory holding environments (not a folder).
pub const ENVIRONMENTS_DIR: &str = "environments";

/// A loaded collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Collection {
    /// Directory containing `snapreq.toml`.
    pub root: PathBuf,
    pub file: CollectionFile,
    pub items: Vec<Item>,
    /// `*.flow.toml` files anywhere in the collection, sorted by key.
    pub flows: Vec<FlowItem>,
}

/// A flow file in the collection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlowItem {
    /// Path without `.flow.toml`, with `/`: `orders/checkout`.
    pub key: String,
    /// Relative to the collection root.
    pub path: PathBuf,
    pub file: FlowFile,
}

impl FlowItem {
    pub fn name(&self) -> &str {
        self.file.name.as_deref().unwrap_or(&self.key)
    }
}

/// A request or a folder, in display order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Item {
    Request(RequestItem),
    Folder(Folder),
}

impl Item {
    pub fn key(&self) -> &str {
        match self {
            Self::Request(r) => &r.key,
            Self::Folder(f) => &f.key,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestItem {
    /// File name without `.toml`.
    pub key: String,
    /// Path of the file relative to the collection root.
    pub path: PathBuf,
    pub file: RequestFile,
}

impl RequestItem {
    /// Display name: the `name` key, or the key.
    pub fn name(&self) -> &str {
        self.file.name.as_deref().unwrap_or(&self.key)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Folder {
    /// Directory name.
    pub key: String,
    /// Path of the directory relative to the collection root.
    pub path: PathBuf,
    /// Contents of `folder.toml` (defaults when absent).
    pub file: FolderFile,
    pub items: Vec<Item>,
}

impl Collection {
    /// Loads the collection in `root`. Invalid files are errors; unknown keys in `order`
    /// lists are warnings.
    pub fn load(root: impl AsRef<Path>) -> Result<(Self, Vec<Warning>)> {
        let root = root.as_ref().to_path_buf();
        let collection_path = root.join(COLLECTION_FILE);
        if !collection_path.is_file() {
            return Err(Error::invalid(
                &collection_path,
                format!("not a SnapRequest collection (no {COLLECTION_FILE})"),
            ));
        }
        let file: CollectionFile = read_toml(&collection_path)?;
        if file.format > FORMAT_VERSION {
            return Err(Error::UnsupportedFormat {
                path: collection_path,
                found: file.format,
                supported: FORMAT_VERSION,
            });
        }
        file.script
            .validate()
            .map_err(|m| Error::invalid(&collection_path, m))?;
        if file.format == 0 {
            return Err(Error::invalid(
                &collection_path,
                "`format` must be 1 or higher",
            ));
        }

        let mut warnings = Vec::new();
        let mut flows = Vec::new();
        let items = load_dir(
            &root,
            Path::new(""),
            &file.order,
            true,
            &mut warnings,
            &mut flows,
        )?;
        flows.sort_by(|a: &FlowItem, b| a.key.cmp(&b.key));
        Ok((
            Self {
                root,
                file,
                items,
                flows,
            },
            warnings,
        ))
    }

    /// Every request in display order, with its slash-separated key path
    /// (`users/create-user`).
    pub fn requests(&self) -> Vec<(String, &RequestItem)> {
        fn walk<'a>(items: &'a [Item], prefix: &str, out: &mut Vec<(String, &'a RequestItem)>) {
            for item in items {
                let key = if prefix.is_empty() {
                    item.key().to_owned()
                } else {
                    format!("{prefix}/{}", item.key())
                };
                match item {
                    Item::Request(r) => out.push((key, r)),
                    Item::Folder(f) => walk(&f.items, &key, out),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, "", &mut out);
        out
    }

    /// The request at a key path such as `users/create-user` (a `.toml` suffix is accepted).
    pub fn find(&self, key_path: &str) -> Option<&RequestItem> {
        let wanted = key_path
            .trim_matches('/')
            .trim_end_matches(".toml")
            .replace('\\', "/");
        self.requests()
            .into_iter()
            .find(|(k, _)| *k == wanted)
            .map(|(_, r)| r)
    }

    /// Replaces the request at `key_path` in memory, or adds it to its (existing) folder,
    /// so unsaved edits can be built and sent as if they were saved. Nothing is written.
    pub fn put_request(
        &mut self,
        key_path: &str,
        file: RequestFile,
    ) -> std::result::Result<(), String> {
        let wanted = key_path.trim_matches('/').trim_end_matches(".toml");
        let mut parts: Vec<&str> = wanted.split('/').collect();
        let name = parts
            .pop()
            .filter(|n| !n.is_empty())
            .ok_or("empty request key")?;
        let mut items = &mut self.items;
        let mut dir = PathBuf::new();
        for part in parts {
            dir.push(part);
            items = match items.iter_mut().find_map(|i| match i {
                Item::Folder(f) if f.key == part => Some(f),
                _ => None,
            }) {
                Some(f) => &mut f.items,
                None => return Err(format!("no folder `{}`", dir.display())),
            };
        }
        let existing = items.iter_mut().find_map(|i| match i {
            Item::Request(r) if r.key == name => Some(r),
            _ => None,
        });
        match existing {
            Some(r) => r.file = file,
            None => items.push(Item::Request(RequestItem {
                key: name.to_owned(),
                path: dir.join(format!("{name}.toml")),
                file,
            })),
        }
        Ok(())
    }

    /// The folders on the way to a request, outermost first (used for inheritance).
    /// The flow with this key (`orders/checkout`, with or without `.flow.toml`).
    pub fn find_flow(&self, key: &str) -> Option<&FlowItem> {
        let wanted = key
            .trim_matches('/')
            .trim_end_matches(FLOW_SUFFIX)
            .replace('\\', "/");
        self.flows.iter().find(|f| f.key == wanted)
    }

    pub fn folders_of(&self, key_path: &str) -> Vec<&Folder> {
        let mut out = Vec::new();
        let mut items = &self.items;
        let wanted = key_path.trim_matches('/').trim_end_matches(".toml");
        let mut parts: Vec<&str> = wanted.split('/').collect();
        parts.pop();
        for part in parts {
            match items.iter().find_map(|i| match i {
                Item::Folder(f) if f.key == part => Some(f),
                _ => None,
            }) {
                Some(f) => {
                    out.push(f);
                    items = &f.items;
                }
                None => break,
            }
        }
        out
    }
}

fn read_toml<T: DeserializeOwned>(path: &Path) -> Result<T> {
    let text = std::fs::read_to_string(path).map_err(|e| Error::io(path, e))?;
    toml::from_str(&text).map_err(|e| Error::toml(path, &text, &e))
}

fn load_dir(
    root: &Path,
    rel: &Path,
    order: &[String],
    top_level: bool,
    warnings: &mut Vec<Warning>,
    flows: &mut Vec<FlowItem>,
) -> Result<Vec<Item>> {
    let dir = root.join(rel);
    let entries = std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))?;
    let mut items = Vec::new();

    for entry in entries {
        let entry = entry.map_err(|e| Error::io(&dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let file_type = entry.file_type().map_err(|e| Error::io(&entry.path(), e))?;
        let rel_path = rel.join(&name);

        if file_type.is_dir() {
            if top_level && name == ENVIRONMENTS_DIR {
                continue;
            }
            let folder_file_path = root.join(&rel_path).join(FOLDER_FILE);
            let folder_file: FolderFile = if folder_file_path.is_file() {
                read_toml(&folder_file_path)?
            } else {
                FolderFile::default()
            };
            folder_file
                .script
                .validate()
                .map_err(|m| Error::invalid(&folder_file_path, m))?;
            let children = load_dir(root, &rel_path, &folder_file.order, false, warnings, flows)?;
            items.push(Item::Folder(Folder {
                key: name,
                path: rel_path,
                file: folder_file,
                items: children,
            }));
        } else if let Some(stem) = name.strip_suffix(FLOW_SUFFIX) {
            let path = root.join(&rel_path);
            let file: FlowFile = read_toml(&path)?;
            file.validate().map_err(|m| Error::invalid(&path, m))?;
            let key = rel.join(stem).to_string_lossy().replace('\\', "/");
            flows.push(FlowItem {
                key,
                path: rel_path,
                file,
            });
        } else if let Some(key) = name.strip_suffix(".toml") {
            if name == COLLECTION_FILE || name == FOLDER_FILE {
                continue;
            }
            let path = root.join(&rel_path);
            let file: RequestFile = read_toml(&path)?;
            file.validate().map_err(|m| Error::invalid(&path, m))?;
            items.push(Item::Request(RequestItem {
                key: key.to_owned(),
                path: rel_path,
                file,
            }));
        }
    }

    apply_order(&mut items, order, &dir, warnings);
    Ok(items)
}

/// Listed keys first, in list order; the rest alphabetically (folders and requests mixed).
fn apply_order(items: &mut [Item], order: &[String], dir: &Path, warnings: &mut Vec<Warning>) {
    for key in order {
        if !items.iter().any(|i| i.key() == key) {
            warnings.push(Warning {
                path: dir.to_owned(),
                message: format!("`order` lists `{key}`, which does not exist"),
            });
        }
    }
    items.sort_by(|a, b| {
        let rank = |i: &Item| {
            order
                .iter()
                .position(|k| k == i.key())
                .unwrap_or(usize::MAX)
        };
        rank(a).cmp(&rank(b)).then_with(|| a.key().cmp(b.key()))
    });
}
