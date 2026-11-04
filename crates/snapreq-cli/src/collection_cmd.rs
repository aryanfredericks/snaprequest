//! Collection support in the CLI: running collection requests, `ls`, `new`, `secret`.

use std::io::{IsTerminal, Read, Write};
use std::path::{Path, PathBuf};

use snapreq_collection::load::COLLECTION_FILE;
use snapreq_collection::save::{save_collection, save_request};
use snapreq_collection::{
    AssertSpec, Captures, Collection, CollectionFile, Environment, Item, RequestFile, Scopes,
    Secrets, Variables, build_assertions, secrets,
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

pub(crate) fn parse_vars(raw: &[String]) -> Result<Variables, ArgError> {
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
/// Builds the request, its `[assert]` checks (empty with `--no-assert`) and its captures.
pub(crate) fn build(root: &Path, key: &str, args: &SendArgs) -> Result<Built, ArgError> {
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
    load_secrets(&collection, args.env.as_deref())
        .map_err(|e| ArgError(e.to_string()))?
        .fill(&mut scopes);

    // Scripts (ADR-031): pre-request scripts run now, test scripts after the response.
    let item = collection
        .find(key)
        .ok_or_else(|| ArgError(format!("no request `{key}` in the collection")))?;
    let item_path = collection.root.join(&item.path);
    let scripts =
        snapreq_test::script::Scripts::for_request(&collection, key, &item.file, &item_path, None);
    let pre = scripts
        .codes(snapreq_script::Phase::PreRequest)
        .map_err(ArgError)?;
    let tests = scripts
        .codes(snapreq_script::Phase::Test)
        .map_err(ArgError)?;
    let mut file = item.file.clone();
    let mut runtime = runtime;
    if !pre.is_empty() {
        let before = snapreq_test::script::script_request(&file);
        let chain = snapreq_test::script::run(
            &pre,
            snapreq_test::script::input(
                snapreq_script::Phase::PreRequest,
                &scopes,
                before.clone(),
                None,
            ),
        );
        let mut hidden = snapreq_test::script::secret_values(&scopes);
        hidden.extend(
            chain
                .vars
                .iter()
                .filter(|v| v.secret)
                .map(|v| v.value.clone()),
        );
        print_logs(&chain.logs, &hidden);
        if let Some((label, e)) = &chain.error {
            return Err(ArgError(snapreq_test::report::hide(
                &format!("{label} pre-request script: {e}"),
                &hidden,
            )));
        }
        for v in &chain.vars {
            if v.secret {
                runtime.set_secret(&v.name, &v.value);
            } else {
                runtime.set(&v.name, &v.value);
            }
        }
        if let Some(after) = &chain.request {
            snapreq_test::script::apply_request(&mut file, &before, after);
            file.validate()
                .map_err(|e| ArgError(format!("after the pre-request script: {e}")))?;
        }
        scopes = Scopes::for_request(
            &collection,
            key,
            environment.as_ref(),
            &Variables::default(),
            &runtime,
        );
        load_secrets(&collection, args.env.as_deref())
            .map_err(|e| ArgError(e.to_string()))?
            .fill(&mut scopes);
    }
    let mut req = snapreq_collection::build_file(&collection, key, &file, &item_path, &scopes)
        .map_err(|e| ArgError(e.to_string()))?;
    for v in runtime.0.iter().filter(|v| v.secret) {
        if let Some(value) = &v.value
            && !req.settings.secrets.contains(value)
        {
            req.settings.secrets.push(value.clone());
        }
    }
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
    let captures = collection
        .find(key)
        .map(|item| item.file.capture.clone())
        .unwrap_or_default();
    // Names of secret values, so an export can read them from `SNAPREQ_SECRET_<NAME>`.
    let named = scopes
        .visible()
        .into_iter()
        .filter(|(v, _)| v.secret || scopes.is_declared_secret(&v.name))
        .filter_map(|(v, _)| Some((v.name.clone(), v.value.clone()?)))
        .collect();
    let tests = (!tests.is_empty()).then(|| TestScripts {
        input: snapreq_test::script::input(
            snapreq_script::Phase::Test,
            &scopes,
            snapreq_test::script::script_request(&file),
            None,
        ),
        hidden: snapreq_test::script::secret_values(&scopes),
        codes: tests,
    });
    Ok((req, checks, captures, named, tests))
}

/// A built request with its checks, captures, named secret values and test scripts.
pub(crate) type Built = (
    Request,
    AssertSpec,
    Captures,
    Vec<(String, String)>,
    Option<TestScripts>,
);

/// Test scripts to run on the response of `snapreq send`.
#[derive(Debug)]
pub(crate) struct TestScripts {
    codes: Vec<(String, String)>,
    input: snapreq_script::Input,
    hidden: Vec<String>,
}

impl TestScripts {
    /// Runs the scripts on `response`; returns their tests as check outcomes.
    pub(crate) fn run(
        mut self,
        response: &snapreq_core::Response,
        secrets: &[String],
    ) -> Vec<snapreq_test::Outcome> {
        self.input.response = Some(snapreq_test::script::script_response(response));
        let chain = snapreq_test::script::run(&self.codes, self.input);
        let mut hidden = self.hidden;
        hidden.extend(secrets.iter().cloned());
        print_logs(&chain.logs, &hidden);
        let mut outcomes = snapreq_test::script::outcomes(&chain);
        snapreq_test::report::scrub_outcomes(&mut outcomes, &hidden);
        outcomes
    }
}

/// `console.log` output of scripts, on stderr.
fn print_logs(logs: &[String], hidden: &[String]) {
    for l in logs {
        eprintln!("log: {}", snapreq_test::report::hide(l, hidden));
    }
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
    if !c.flows.is_empty() {
        let flows: Vec<&str> = c.flows.iter().map(|f| f.key.as_str()).collect();
        let gap = if envs.is_empty() { "\n" } else { "" };
        writeln!(out, "{gap}flows: {}", flows.join(", ")).map_err(io)?;
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

/// The key for sealed secret values (written by a server): `SNAPREQ_SECRET_KEY`, or the key file
/// of the local server's data directory (`SNAPREQ_DATA_DIR` or the platform default).
pub(crate) fn secret_key() -> Option<snapreq_collection::sealed::SecretKey> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let dir = env("SNAPREQ_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| snapreq_server::config::default_data_dir(env));
    match snapreq_collection::sealed::find(
        env(snapreq_collection::sealed::KEY_ENV).as_deref(),
        dir.as_deref(),
    ) {
        Ok(key) => key,
        Err(e) => {
            eprintln!("warning: {e}");
            None
        }
    }
}

/// The secret values of a collection; warns about sealed values that cannot be opened.
pub(crate) fn load_secrets(
    collection: &Collection,
    env: Option<&str>,
) -> snapreq_collection::Result<Secrets> {
    let s = Secrets::load_with_key(collection, env, std::env::vars(), secret_key().as_ref())?;
    if !s.locked().is_empty() {
        eprintln!(
            "warning: sealed secret values cannot be opened ({}): set {} to the server's key",
            s.locked().join(", "),
            snapreq_collection::sealed::KEY_ENV
        );
    }
    Ok(s)
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
    // Sealed only when a key is given explicitly; the file itself is owner-only and gitignored.
    let key = std::env::var(snapreq_collection::sealed::KEY_ENV)
        .ok()
        .map(|k| snapreq_collection::sealed::SecretKey::decode(&k))
        .transpose()
        .map_err(|e| ArgError(format!("{}: {e}", snapreq_collection::sealed::KEY_ENV)))?;
    secrets::store(&root, env, name, &value, key.as_ref()).map_err(|e| ArgError(e.to_string()))
}
