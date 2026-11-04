//! Backup and restore of a server's state (PLAN P8.6, ADR-028).
//!
//! A backup is a gzip-compressed tar archive:
//!
//! ```text
//! manifest.json        format, SnapRequest version, schema version, what is included
//! data/snapreq.db      a consistent snapshot of the database (`VACUUM INTO`, safe while serving)
//! data/config.toml     if present
//! data/secret.key      only with `include_key` (otherwise sealed secrets need the key kept elsewhere)
//! workspace/…          every workspace and collection, `.secrets` included (values sealed)
//! ```
//!
//! Restoring unpacks into an empty data directory and workspace (or replaces them with `force`);
//! the server must not be running. A database from an older version is migrated at the next start.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{CONFIG_FILE, Config};
use crate::store::{DB_FILE, Store, db_err};
use crate::{Error, Result};

/// Version of the archive layout.
pub const FORMAT: u32 = 1;
const MANIFEST: &str = "manifest.json";

/// What an archive holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub snapreq: String,
    pub schema: u32,
    /// Unix milliseconds.
    pub created_at: i64,
    pub includes_key: bool,
    /// Files under `workspace/`.
    pub workspace_files: usize,
}

fn io(context: String) -> impl FnOnce(std::io::Error) -> Error {
    move |source| Error::Io { context, source }
}

impl Store {
    /// Writes a consistent copy of the database to `path` (which must not exist).
    pub async fn snapshot(&self, path: &Path) -> Result<()> {
        let target = path.to_string_lossy().into_owned();
        self.with(move |c| {
            c.execute("VACUUM INTO ?1", [target])
                .map(drop)
                .map_err(db_err)
        })
        .await
    }
}

/// Every file below `dir`, as paths relative to it (sorted; symbolic links are skipped).
fn files_below(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel) = stack.pop() {
        let entries = match std::fs::read_dir(dir.join(&rel)) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
            Err(e) => return Err(io(format!("cannot read `{}`", dir.join(&rel).display()))(e)),
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let kind = entry
                .file_type()
                .map_err(io(format!("cannot read `{}`", entry.path().display())))?;
            let path = rel.join(entry.file_name());
            if kind.is_dir() {
                stack.push(path);
            } else if kind.is_file() {
                out.push(path);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Writes a backup of `config`'s data directory and workspace to `out`.
pub async fn create(
    config: &Config,
    store: &Store,
    out: impl Write,
    include_key: bool,
) -> Result<Manifest> {
    let tmp = config
        .data_dir
        .join(format!(".backup-{}.db", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    store.snapshot(&tmp).await?;
    let result = write_archive(config, store, &tmp, out, include_key).await;
    let _ = std::fs::remove_file(&tmp);
    result
}

async fn write_archive(
    config: &Config,
    store: &Store,
    db: &Path,
    out: impl Write,
    include_key: bool,
) -> Result<Manifest> {
    let workspace = files_below(&config.workspace)?;
    let manifest = Manifest {
        format: FORMAT,
        snapreq: snapreq_core::VERSION.to_owned(),
        schema: store.schema_version().await?,
        created_at: crate::store::now_ms(),
        includes_key: include_key,
        workspace_files: workspace.len(),
    };
    let gz = flate2::write::GzEncoder::new(out, flate2::Compression::default());
    let mut tar = tar::Builder::new(gz);
    tar.mode(tar::HeaderMode::Deterministic);
    let fail = |what: &str| io(format!("cannot write the backup ({what})"));
    let text = serde_json::to_vec_pretty(&manifest).unwrap_or_default();
    let mut header = tar::Header::new_gnu();
    header.set_size(text.len() as u64);
    header.set_mode(0o600);
    tar.append_data(&mut header, MANIFEST, text.as_slice())
        .map_err(fail(MANIFEST))?;
    tar.append_path_with_name(db, format!("data/{DB_FILE}"))
        .map_err(fail(DB_FILE))?;
    let mut extra = vec![CONFIG_FILE];
    if include_key {
        extra.push(snapreq_collection::sealed::KEY_FILE);
    }
    for name in extra {
        let path = config.data_dir.join(name);
        if path.is_file() {
            tar.append_path_with_name(&path, format!("data/{name}"))
                .map_err(fail(name))?;
        }
    }
    for rel in &workspace {
        let name = format!("workspace/{}", rel.to_string_lossy().replace('\\', "/"));
        tar.append_path_with_name(config.workspace.join(rel), &name)
            .map_err(fail(&name))?;
    }
    tar.into_inner()
        .and_then(flate2::write::GzEncoder::finish)
        .map_err(fail("finishing"))?
        .flush()
        .map_err(fail("flushing"))?;
    Ok(manifest)
}

/// Whether `dir` holds anything.
fn has_content(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_some())
}

/// Restores a backup into `config`'s data directory and workspace. Without `force` both must be
/// empty or missing; with it, the database, config, key and workspace are replaced.
pub fn restore(config: &Config, input: impl Read, force: bool) -> Result<Manifest> {
    let has_db = config.data_dir.join(DB_FILE).is_file();
    if !force && (has_db || has_content(&config.workspace)) {
        return Err(Error::Config(format!(
            "`{}` or `{}` already holds data: restore into empty directories, or pass --force to replace it (stop the server first)",
            config.data_dir.display(),
            config.workspace.display()
        )));
    }
    // Unpack aside first, so a broken archive changes nothing.
    std::fs::create_dir_all(&config.data_dir)
        .map_err(io(format!("cannot create `{}`", config.data_dir.display())))?;
    let staging = config
        .data_dir
        .join(format!(".restore-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(io(format!("cannot create `{}`", staging.display())))?;
    let result = unpack_and_move(config, input, &staging);
    let _ = std::fs::remove_dir_all(&staging);
    result
}

fn unpack_and_move(config: &Config, input: impl Read, staging: &Path) -> Result<Manifest> {
    let bad = |why: String| Error::Config(format!("not a SnapRequest backup: {why}"));
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(input));
    let entries = archive.entries().map_err(|e| bad(e.to_string()))?;
    for entry in entries {
        let mut entry = entry.map_err(|e| bad(e.to_string()))?;
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir()) {
            return Err(bad("it contains links or special files".into()));
        }
        // `unpack_in` refuses absolute paths and `..`.
        if !entry.unpack_in(staging).map_err(|e| bad(e.to_string()))? {
            return Err(bad("it contains paths outside the archive".into()));
        }
    }
    let manifest: Manifest = std::fs::read(staging.join(MANIFEST))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .ok_or_else(|| bad("no manifest".into()))?;
    if manifest.format > FORMAT {
        return Err(bad(format!(
            "it was written by a newer SnapRequest (format {})",
            manifest.format
        )));
    }
    if !staging.join("data").join(DB_FILE).is_file() {
        return Err(bad("no database".into()));
    }
    // Replace the database (and its WAL files), config and key.
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(config.data_dir.join(format!("{DB_FILE}{suffix}")));
    }
    for name in [DB_FILE, CONFIG_FILE, snapreq_collection::sealed::KEY_FILE] {
        let from = staging.join("data").join(name);
        if from.is_file() {
            let to = config.data_dir.join(name);
            std::fs::copy(&from, &to).map_err(io(format!("cannot write `{}`", to.display())))?;
            restrict(&to);
        }
    }
    if config.workspace.exists() {
        std::fs::remove_dir_all(&config.workspace).map_err(io(format!(
            "cannot replace `{}`",
            config.workspace.display()
        )))?;
    }
    copy_tree(&staging.join("workspace"), &config.workspace)?;
    Ok(manifest)
}

fn copy_tree(from: &Path, to: &Path) -> Result<()> {
    std::fs::create_dir_all(to).map_err(io(format!("cannot create `{}`", to.display())))?;
    for rel in files_below(from)? {
        let target = to.join(&rel);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(io(format!("cannot create `{}`", parent.display())))?;
        }
        std::fs::copy(from.join(&rel), &target)
            .map_err(io(format!("cannot write `{}`", target.display())))?;
        if rel
            .components()
            .any(|c| c.as_os_str() == snapreq_collection::secrets::SECRETS_DIR)
        {
            restrict(&target);
        }
    }
    Ok(())
}

/// Owner-only permissions for restored state (Unix).
fn restrict(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}
