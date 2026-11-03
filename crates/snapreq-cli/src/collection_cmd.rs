//! Collection support in the CLI: running collection requests, `ls`, `new`, `secret`.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use snapreq_collection::load::COLLECTION_FILE;
use snapreq_collection::save::{save_collection, save_request};
use snapreq_collection::{
    AssertSpec, Collection, CollectionFile, Environment, Item, RequestFile, Scopes, Secrets,
    Variables, build_assertions, build_request, secrets,
};
use snapreq_core::Request;

use crate::cli::SendArgs;
use crate::send::{ArgError, apply_overrides};

/// What `snapreq send` was asked to run.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Target {
    /// `[METHOD] URL`.
    Url,
    /// A request in a collection.
    Collection { root: PathBuf, key: String },
}

/// The nearest directory at or above `start` that contains `snapreq.toml`.
pub(crate) fn find_root(start: &Path) -> Option<PathBuf> {
    let start = std::path::absolute(start).unwrap_or_else(|_| start.to_path_buf());
    start
        .ancestors()
        .find(|dir| dir.join(COLLECTION_FILE).is_file())
        .map(Path::to_path_buf)
}

/// A request key from a path relative to the collection root: `users/create-user`.
fn key_from(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    s.trim_start_matches("./")
        .trim_end_matches(".toml")
        .to_owned()
}

/// Decides whether `args` names a URL or a collection request.
pub(crate) fn resolve_target(args: &SendArgs) -> Result<Target, ArgError> {
    if let Some(dir) = &args.collection {
        let [key] = args.target.as_slice() else {
            return Err(ArgError(
                "with --collection, give one request key such as `users/create-user`".into(),
            ));
        };
        return Ok(Target::Collection {
            root: dir.clone(),
            key: key_from(Path::new(key)),
        });
    }
    if let [single] = args.target.as_slice() {
        let path = Path::new(single);
        if single.ends_with(".toml") && path.is_file() {
            let file = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
            let root = file.parent().and_then(find_root).ok_or_else(|| {
                ArgError(format!(
                    "`{single}` is not inside a collection (no {COLLECTION_FILE} above it)"
                ))
            })?;
            let rel = file.strip_prefix(&root).unwrap_or(&file);
            return Ok(Target::Collection {
                root,
                key: key_from(rel),
            });
        }
        // `snapreq send users/create-user` from inside a collection.
        if !single.contains("://")
            && let Some(root) = find_root(Path::new("."))
            && let Ok((c, _)) = Collection::load(&root)
        {
            let key = key_from(path);
            if c.find(&key).is_some() {
                return Ok(Target::Collection { root, key });
            }
        }
    }
    if args.env.is_some() || !args.vars.is_empty() {
        return Err(ArgError(
            "--env and --var only apply to collection requests".into(),
        ));
    }
    Ok(Target::Url)
}

fn parse_vars(raw: &[String]) -> Result<Variables, ArgError> {
    let mut vars = Variables::default();
    for v in raw {
        let (name, value) = v
            .split_once('=')
            .ok_or_else(|| ArgError(format!("invalid --var `{v}`, expected `name=value`")))?;
        if !snapreq_collection::vars::valid_name(name) {
            return Err(ArgError(format!("invalid variable name `{name}` in --var")));
        }
        vars.set(name, value);
    }
    Ok(vars)
}

/// Builds the collection request `key`, then applies command-line overrides.
/// Collection warnings are printed to stderr.
/// Builds the request and its `[assert]` checks (empty with `--no-assert`).
pub(crate) fn build(
    root: &Path,
    key: &str,
    args: &SendArgs,
) -> Result<(Request, AssertSpec), ArgError> {
    let (collection, warnings) = Collection::load(root).map_err(|e| ArgError(e.to_string()))?;
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    let environment = args
        .env
        .as_deref()
        .map(|e| Environment::load(&collection, e))
        .transpose()
        .map_err(|e| ArgError(e.to_string()))?;
    let runtime = parse_vars(&args.vars)?;
    let mut scopes = Scopes::for_request(
        &collection,
        key,
        environment.as_ref(),
        &Variables::default(),
        &runtime,
    );
    Secrets::load(&collection, args.env.as_deref(), std::env::vars())
        .map_err(|e| ArgError(e.to_string()))?
        .fill(&mut scopes);

    let mut req = build_request(&collection, key, &scopes).map_err(|e| ArgError(e.to_string()))?;
    // A value given with --var for a variable declared secret is hidden too.
    for v in &runtime.0 {
        if let Some(value) = &v.value
            && scopes.is_declared_secret(&v.name)
            && !req.settings.secrets.contains(value)
        {
            req.settings.secrets.push(value.clone());
        }
    }
    apply_overrides(&mut req, args)?;
    let checks = if args.no_assert {
        AssertSpec::default()
    } else {
        build_assertions(&collection, key, &scopes).map_err(|e| ArgError(e.to_string()))?
    };
    Ok((req, checks))
}

/// `snapreq ls`.
pub(crate) fn ls(dir: Option<&Path>, out: &mut dyn Write) -> Result<(), ArgError> {
    let start = dir.unwrap_or(Path::new("."));
    let root = find_root(start).ok_or_else(|| {
        ArgError(format!(
            "`{}` is not inside a collection (no {COLLECTION_FILE} found)",
            start.display()
        ))
    })?;
    let (c, warnings) = Collection::load(&root).map_err(|e| ArgError(e.to_string()))?;
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    let io = |e: std::io::Error| ArgError(e.to_string());
    writeln!(out, "{}{}", c.file.name, auth_note(c.file.auth.as_ref())).map_err(io)?;
    /// `  [auth: bearer]` where a file sets its own auth; inheriting items show nothing.
    fn auth_note(auth: Option<&snapreq_collection::AuthSpec>) -> String {
        auth.filter(|a| **a != snapreq_collection::AuthSpec::Inherit)
            .map(|a| format!("  [auth: {}]", a.summary()))
            .unwrap_or_default()
    }
    fn walk(items: &[Item], prefix: &str, out: &mut dyn Write) -> std::io::Result<()> {
        for (i, item) in items.iter().enumerate() {
            let last = i + 1 == items.len();
            let branch = if last { "└── " } else { "├── " };
            match item {
                Item::Request(r) => {
                    let name = r
                        .file
                        .name
                        .as_deref()
                        .map(|n| format!("  {n}"))
                        .unwrap_or_default();
                    writeln!(
                        out,
                        "{prefix}{branch}{:<7} {}{name}{}",
                        r.file.method(),
                        r.key,
                        auth_note(r.file.auth.as_ref())
                    )?;
                }
                Item::Folder(f) => {
                    let name = f
                        .file
                        .name
                        .as_deref()
                        .map(|n| format!("  {n}"))
                        .unwrap_or_default();
                    writeln!(
                        out,
                        "{prefix}{branch}{}/{name}{}",
                        f.key,
                        auth_note(f.file.auth.as_ref())
                    )?;
                    let next = format!("{prefix}{}", if last { "    " } else { "│   " });
                    walk(&f.items, &next, out)?;
                }
            }
        }
        Ok(())
    }
    walk(&c.items, "", out).map_err(io)?;
    let envs = Environment::list(&c).map_err(|e| ArgError(e.to_string()))?;
    if !envs.is_empty() {
        writeln!(out, "\nenvironments: {}", envs.join(", ")).map_err(io)?;
    }
    Ok(())
}

/// `snapreq new collection`.
pub(crate) fn new_collection(dir: &Path, name: Option<&str>) -> Result<PathBuf, ArgError> {
    let file = dir.join(COLLECTION_FILE);
    if file.exists() {
        return Err(ArgError(format!("`{}` already exists", file.display())));
    }
    let name = name.map(str::to_owned).unwrap_or_else(|| {
        std::path::absolute(dir)
            .ok()
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "My API".into())
    });
    save_collection(&file, &CollectionFile::new(name)).map_err(|e| ArgError(e.to_string()))?;
    Ok(file)
}

/// `snapreq new request`.
pub(crate) fn new_request(
    path: &Path,
    target: &[String],
    name: Option<&str>,
) -> Result<PathBuf, ArgError> {
    if path.extension().is_none_or(|e| e != "toml") {
        return Err(ArgError("request files end in `.toml`".into()));
    }
    if path.exists() {
        return Err(ArgError(format!("`{}` already exists", path.display())));
    }
    // The file's directory may not exist yet: look for the collection above it.
    let parent = path.parent().unwrap_or(Path::new("."));
    let existing = std::path::absolute(parent)
        .unwrap_or_else(|_| parent.to_path_buf())
        .ancestors()
        .find(|p| p.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_default();
    if find_root(&existing).is_none() {
        return Err(ArgError(format!(
            "`{}` is not inside a collection; create one with `snapreq new collection DIR`",
            path.display()
        )));
    }
    let (method, url) = match target {
        [url] => ("GET", url.as_str()),
        [method, url] => (method.as_str(), url.as_str()),
        _ => return Err(ArgError("expected `[METHOD] URL`".into())),
    };
    http::Method::from_bytes(method.to_ascii_uppercase().as_bytes())
        .map_err(|_| ArgError(format!("invalid HTTP method `{method}`")))?;
    let mut file = RequestFile::new(method, url);
    file.name = name.map(str::to_owned);
    save_request(path, &file).map_err(|e| ArgError(e.to_string()))?;
    Ok(path.to_path_buf())
}

/// `snapreq secret set`.
pub(crate) fn secret_set(
    name: &str,
    env: Option<&str>,
    collection: Option<&Path>,
) -> Result<PathBuf, ArgError> {
    let start = collection.unwrap_or(Path::new("."));
    let root = find_root(start)
        .ok_or_else(|| ArgError(format!("`{}` is not inside a collection", start.display())))?;
    let value = if std::io::stdin().is_terminal() {
        rpassword::prompt_password(format!("Value for `{name}`: "))
            .map_err(|e| ArgError(format!("cannot read the value: {e}")))?
    } else {
        let mut s = String::new();
        std::io::stdin()
            .read_to_string(&mut s)
            .map_err(|e| ArgError(format!("cannot read the value from stdin: {e}")))?;
        s.trim_end_matches(['\r', '\n']).to_owned()
    };
    if value.is_empty() {
        return Err(ArgError("the value is empty".into()));
    }
    secrets::store(&root, env, name, &value).map_err(|e| ArgError(e.to_string()))
}
