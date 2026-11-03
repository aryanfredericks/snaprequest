//! Writing collection files in a stable, diff-friendly form.

use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};
use crate::files::{CollectionFile, FolderFile, RequestFile};

/// Serializes a file to TOML text. Field order is fixed by the types, so saving the
/// same data always produces the same text.
pub fn to_toml<T: Serialize>(value: &T) -> Result<String> {
    let mut doc = toml_edit::ser::to_document(value).map_err(|e| Error::Invalid {
        path: Default::default(),
        location: None,
        message: format!("cannot serialize: {e}"),
    })?;
    layout(doc.as_table_mut());
    Ok(doc.to_string())
}

/// Lays the document out the way people write these files by hand:
/// - nested tables become `[sections]` and lists of tables become `[[sections]]`;
/// - entries inside key/value tables (`[headers]`, `[query]`, `[body.fields]`) stay inline,
///   because an entry written as a sub-table (`[headers.X]`) would have to come after every
///   plain entry and change the order.
fn layout(table: &mut toml_edit::Table) {
    use toml_edit::{ArrayOfTables, Item, Value};
    for (key, item) in table.iter_mut() {
        let key_value_table = matches!(
            key.get(),
            "headers" | "query" | "fields" | "vars" | "params"
        );
        if let Item::Value(Value::InlineTable(t)) = item {
            *item = Item::Table(std::mem::take(t).into_table());
        }
        if let Item::Value(Value::Array(a)) = item
            && !a.is_empty()
            && a.iter().all(Value::is_inline_table)
        {
            let mut tables = ArrayOfTables::new();
            for v in a.iter() {
                if let Some(t) = v.as_inline_table() {
                    tables.push(t.clone().into_table());
                }
            }
            *item = Item::ArrayOfTables(tables);
        }
        match item {
            Item::Table(sub) if key_value_table => {
                for (_, entry) in sub.iter_mut() {
                    if let Item::Table(t) = entry {
                        let inline = std::mem::take(t).into_inline_table();
                        *entry = Item::Value(Value::InlineTable(inline));
                    }
                }
            }
            Item::Table(sub) => layout(sub),
            Item::ArrayOfTables(array) => {
                for t in array.iter_mut() {
                    layout(t);
                }
            }
            _ => {}
        }
    }
}

fn write(path: &Path, text: &str) -> Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| Error::io(dir, e))?;
    }
    std::fs::write(path, text).map_err(|e| Error::io(path, e))
}

/// Writes a request file.
pub fn save_request(path: &Path, file: &RequestFile) -> Result<()> {
    write(path, &to_toml(file)?)
}

/// Writes `snapreq.toml`.
pub fn save_collection(path: &Path, file: &CollectionFile) -> Result<()> {
    write(path, &to_toml(file)?)
}

/// Writes an environment file (`environments/<name>.toml`): a top-level variable table.
/// Secret values are never written, only their `{ secret = true }` markers.
pub fn save_environment(path: &Path, vars: &crate::vars::Variables) -> Result<()> {
    let mut doc = toml_edit::ser::to_document(vars).map_err(|e| Error::Invalid {
        path: path.to_owned(),
        location: None,
        message: format!("cannot serialize: {e}"),
    })?;
    // Entries stay inline at the top level, like in `[vars]` tables.
    for (_, item) in doc.as_table_mut().iter_mut() {
        if let toml_edit::Item::Table(t) = item {
            let inline = std::mem::take(t).into_inline_table();
            *item = toml_edit::Item::Value(toml_edit::Value::InlineTable(inline));
        }
    }
    write(path, &doc.to_string())
}

/// Writes `folder.toml`.
pub fn save_folder(path: &Path, file: &FolderFile) -> Result<()> {
    write(path, &to_toml(file)?)
}
