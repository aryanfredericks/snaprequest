//! `snapreq run`: a flow, a folder or a whole collection, optionally per data row.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anstyle::{AnsiColor, Style};
use serde_json::{Value, json};
use snapreq_collection::{Collection, Environment, FLOW_SUFFIX, Scopes, Secrets};
use snapreq_core::{Error, Request};
use snapreq_test::{
    Iteration, Plan, RequestRun, RunOptions, RunOutcome, Unit, UnitRun, data, execute, request_keys,
};

use crate::cli::{ColorArg, FormatArg, RunArgs};
use crate::collection_cmd::{find_root, parse_vars};
use crate::send::ArgError;
use crate::{assert_view, exit, json_out, trace_view};

const PASS: Style = AnsiColor::Green.on_default();
const FAIL: Style = AnsiColor::Red.on_default().bold();
const DIM: Style = Style::new().dimmed();

/// What `snapreq run` was pointed at, before the collection is loaded.
enum Target {
    /// A flow key, or a key that may be a flow or a folder.
    Key(String),
    /// A folder (`None`: the whole collection).
    Folder(Option<String>),
}

fn relative_key(root: &Path, path: &Path) -> String {
    let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    abs.strip_prefix(root)
        .unwrap_or(&abs)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The collection root and what to run.
fn resolve(args: &RunArgs) -> Result<(PathBuf, Target), ArgError> {
    let not_in = |p: &Path| ArgError(format!("`{}` is not inside a collection", p.display()));
    if let Some(dir) = &args.collection {
        let root = find_root(dir).ok_or_else(|| not_in(dir))?;
        return Ok((
            root,
            args.target
                .clone()
                .map_or(Target::Folder(None), Target::Key),
        ));
    }
    let Some(target) = &args.target else {
        let cwd = Path::new(".");
        let root = find_root(cwd).ok_or_else(|| {
            ArgError(
                "the current directory is not inside a collection (give a TARGET or -c DIR)".into(),
            )
        })?;
        return Ok((root, Target::Folder(None)));
    };
    let path = Path::new(target);
    if target.ends_with(FLOW_SUFFIX) && path.is_file() {
        let root = std::path::absolute(path)
            .ok()
            .and_then(|p| p.parent().and_then(find_root))
            .ok_or_else(|| not_in(path))?;
        let key = relative_key(&root, path);
        return Ok((
            root,
            Target::Key(key.trim_end_matches(FLOW_SUFFIX).to_owned()),
        ));
    }
    if path.is_dir() {
        let root = find_root(path).ok_or_else(|| not_in(path))?;
        let key = relative_key(&root, path);
        return Ok((root, Target::Folder((!key.is_empty()).then_some(key))));
    }
    let root = find_root(Path::new(".")).ok_or_else(|| {
        ArgError(format!(
            "`{target}` is not a flow file or directory, and the current directory is not inside a collection (use -c DIR)"
        ))
    })?;
    Ok((root, Target::Key(target.clone())))
}

/// Turns the target into a unit of the loaded collection.
fn unit<'c>(c: &'c Collection, target: Target, folder: Option<&str>) -> Result<Unit<'c>, String> {
    match target {
        Target::Key(key) => {
            if let Some(flow) = c.find_flow(&key) {
                if folder.is_some() {
                    return Err("--folder cannot be used with a flow".into());
                }
                return Ok(Unit::Flow(flow));
            }
            request_keys(c, Some(&key))
                .map(Unit::Requests)
                .map_err(|_| {
                    let flows: Vec<&str> = c.flows.iter().map(|f| f.key.as_str()).collect();
                    format!(
                        "no flow or folder `{key}` in the collection (flows: {})",
                        if flows.is_empty() {
                            "none".into()
                        } else {
                            flows.join(", ")
                        }
                    )
                })
        }
        Target::Folder(dir) => match (dir, folder) {
            (Some(_), Some(_)) => Err("give the folder either as TARGET or with --folder".into()),
            (d, f) => request_keys(c, d.as_deref().or(f)).map(Unit::Requests),
        },
    }
}

pub(crate) async fn run(args: &RunArgs) -> u8 {
    let json = args.format == FormatArg::Json;
    let invalid = |message: String| {
        if json {
            crate::emit_json(&json_out::invalid_arguments(&message));
        } else {
            eprintln!("error: {message}");
        }
        exit::INVALID_REQUEST
    };
    let (root, target) = match resolve(args) {
        Ok(r) => r,
        Err(e) => return invalid(e.to_string()),
    };
    let (collection, warnings) = match Collection::load(&root) {
        Ok(c) => c,
        Err(e) => return invalid(e.to_string()),
    };
    for w in &warnings {
        eprintln!("warning: {w}");
    }
    let unit = match unit(&collection, target, args.folder.as_deref()) {
        Ok(u) => u,
        Err(e) => return invalid(e),
    };
    let environment = match args
        .env
        .as_deref()
        .map(|e| Environment::load(&collection, e))
        .transpose()
    {
        Ok(e) => e,
        Err(e) => return invalid(e.to_string()),
    };
    let secrets = match Secrets::load(&collection, args.env.as_deref(), std::env::vars()) {
        Ok(s) => s,
        Err(e) => return invalid(e.to_string()),
    };
    let overrides = match parse_vars(&args.vars) {
        Ok(v) => v,
        Err(e) => return invalid(e.to_string()),
    };
    let rows = match &args.data {
        Some(path) => match data::load(path) {
            Ok(rows) => rows,
            Err(e) => return invalid(e),
        },
        None => Vec::new(),
    };
    if args.parallel == 0 {
        return invalid("--parallel must be at least 1".into());
    }
    if args.iterations == Some(0) {
        return invalid("--iterations must be at least 1".into());
    }
    let timeout = match args.timeout {
        Some(t) if t.is_finite() && t >= 0.0 => (t > 0.0).then(|| Duration::from_secs_f64(t)),
        Some(_) => return invalid("--timeout must be a non-negative number of seconds".into()),
        None => None,
    };

    // Values of variables declared secret are hidden, whether they come from --var or data.
    let scope_key = match &unit {
        Unit::Flow(f) => f.key.clone(),
        Unit::Requests(keys) => keys[0].clone(),
    };
    let declared = Scopes::for_request(
        &collection,
        &scope_key,
        environment.as_ref(),
        &Default::default(),
        &Default::default(),
    );
    let mut hidden: Vec<String> = args.secrets.clone();
    for v in overrides.0.iter().chain(rows.iter().flat_map(|r| &r.0)) {
        if let Some(value) = &v.value
            && declared.is_declared_secret(&v.name)
        {
            hidden.push(value.clone());
        }
    }
    let plan = Plan {
        collection: &collection,
        environment: environment.as_ref(),
        secrets: &secrets,
        client: crate::client(),
        overrides,
        hidden: hidden.clone(),
        rows,
        iterations: args.iterations.unwrap_or(1),
        options: RunOptions {
            bail: args.bail,
            delay: args.delay.map(Duration::from_millis),
            parallel: args.parallel,
        },
    };
    let planned = if plan.rows.is_empty() {
        plan.iterations
    } else {
        plan.rows.len()
    };
    let adjust = |req: &mut Request| {
        if args.insecure {
            req.settings.tls.verify = false;
        }
        if args.timeout.is_some() {
            req.settings.timeout = timeout;
        }
        if args.verbose >= 2 {
            req.settings.capture_wire = true;
        }
    };
    let iterations = execute(&plan, &unit, adjust).await;
    let info = Info::new(&collection, &unit);

    if json {
        crate::emit_json(&json_doc(&info, &iterations, planned, &declared));
    } else {
        let color = match args.color {
            ColorArg::Auto => anstream::ColorChoice::Auto,
            ColorArg::Always => anstream::ColorChoice::Always,
            ColorArg::Never => anstream::ColorChoice::Never,
        };
        let mut out = anstream::AutoStream::new(io::stdout().lock(), color);
        if let Err(e) = render(
            &mut out,
            &info,
            &iterations,
            planned,
            args.verbose,
            &declared,
        ) && e.kind() != io::ErrorKind::BrokenPipe
        {
            eprintln!("error: {e}");
            return exit::INTERNAL;
        }
    }
    exit_code(&iterations)
}

/// Header information about the unit.
struct Info {
    kind: &'static str,
    key: String,
    name: String,
}

impl Info {
    fn new(c: &Collection, unit: &Unit<'_>) -> Self {
        match unit {
            Unit::Flow(f) => Self {
                kind: "flow",
                key: f.key.clone(),
                name: f.name().to_owned(),
            },
            Unit::Requests(keys) => {
                let all = c.requests().len();
                let folder = if keys.len() == all {
                    String::new()
                } else {
                    keys[0]
                        .rsplit_once('/')
                        .map(|(f, _)| f.to_owned())
                        .unwrap_or_default()
                };
                Self {
                    kind: if folder.is_empty() {
                        "collection"
                    } else {
                        "folder"
                    },
                    name: c.file.name.clone(),
                    key: folder,
                }
            }
        }
    }
}

/// The worst failure decides: a request that could not be built, then network trouble,
/// then failed checks.
fn exit_code(iterations: &[Iteration]) -> u8 {
    let runs: Vec<&RequestRun> = iterations.iter().flat_map(Iteration::requests).collect();
    if runs
        .iter()
        .any(|r| matches!(r.outcome, RunOutcome::Invalid(_)))
    {
        return exit::INVALID_REQUEST;
    }
    let failed = |timeout: bool| {
        runs.iter().any(|r| match &r.outcome {
            RunOutcome::Failed(Error::Execution(e)) => e.timed_out == timeout,
            RunOutcome::Failed(_) => !timeout,
            _ => false,
        })
    };
    if failed(false) {
        exit::NETWORK
    } else if failed(true) {
        exit::TIMEOUT
    } else if iterations.iter().all(Iteration::passed) {
        exit::OK
    } else {
        exit::ASSERTION_FAILED
    }
}

/// One line of output: a flow step or a request.
struct Line<'a> {
    number: usize,
    title: &'a str,
    attempts: u32,
    run: &'a RequestRun,
}

fn lines(it: &Iteration) -> Vec<Line<'_>> {
    match &it.result {
        UnitRun::Flow(f) => f
            .steps
            .iter()
            .map(|s| Line {
                number: s.number,
                title: &s.title,
                attempts: s.attempts,
                run: &s.run,
            })
            .collect(),
        UnitRun::Requests { runs, .. } => runs
            .iter()
            .enumerate()
            .map(|(i, r)| Line {
                number: i + 1,
                title: &r.key,
                attempts: 1,
                run: r,
            })
            .collect(),
    }
}

/// Total and skipped lines of an iteration.
fn counts(it: &Iteration) -> (usize, usize) {
    match &it.result {
        UnitRun::Flow(f) => (f.total, f.skipped()),
        UnitRun::Requests { runs, total } => (*total, total - runs.len()),
    }
}

/// `METHOD url` with secrets removed.
fn request_line(run: &RequestRun) -> String {
    match &run.request {
        Some(req) => {
            let url = req
                .effective_url()
                .map_or_else(|_| req.url.clone(), |u| u.to_string());
            format!("{} {}", req.method, json_out::scrub(req, &url))
        }
        None => String::new(),
    }
}

fn secrets_of(run: &RequestRun) -> Vec<String> {
    run.request
        .as_ref()
        .map(Request::secrets)
        .unwrap_or_default()
}

/// `user=alice, role=admin`, with declared secrets hidden.
fn row_text(it: &Iteration, declared: &Scopes) -> Option<String> {
    let row = it.data.as_ref()?;
    Some(
        row.0
            .iter()
            .map(|v| {
                let value = if declared.is_declared_secret(&v.name) {
                    "<redacted>"
                } else {
                    v.value.as_deref().unwrap_or_default()
                };
                format!("{}={value}", v.name)
            })
            .collect::<Vec<_>>()
            .join(", "),
    )
}

fn render(
    out: &mut dyn Write,
    info: &Info,
    iterations: &[Iteration],
    planned: usize,
    verbose: u8,
    declared: &Scopes,
) -> io::Result<()> {
    let what = match info.kind {
        "collection" => "collection".to_owned(),
        _ => info.key.clone(),
    };
    writeln!(out, "{}  {DIM}{what}{DIM:#}", info.name)?;
    let many = planned > 1;
    for it in iterations {
        if many {
            let row = row_text(it, declared)
                .map(|r| format!("  {DIM}{r}{DIM:#}"))
                .unwrap_or_default();
            writeln!(out, "iteration {}/{planned}{row}", it.number)?;
        }
        render_iteration(out, info, it, verbose)?;
    }
    if many {
        let failed = iterations.iter().filter(|i| !i.passed()).count();
        let skipped = planned - iterations.len();
        let skipped = if skipped > 0 {
            format!(", {skipped} skipped")
        } else {
            String::new()
        };
        if failed == 0 && skipped.is_empty() {
            writeln!(out, "{PASS}all {planned} iterations passed{PASS:#}")?;
        } else {
            writeln!(
                out,
                "{FAIL}{failed} of {planned} iterations failed{skipped}{FAIL:#}"
            )?;
        }
    }
    Ok(())
}

fn render_iteration(
    out: &mut dyn Write,
    info: &Info,
    it: &Iteration,
    verbose: u8,
) -> io::Result<()> {
    let lines = lines(it);
    let width = lines
        .iter()
        .map(|l| l.title.chars().count())
        .max()
        .unwrap_or(0);
    for line in &lines {
        let passed = line.run.passed();
        let mark = if passed {
            format!("{PASS}✓{PASS:#}")
        } else {
            format!("{FAIL}✗{FAIL:#}")
        };
        let result = match &line.run.outcome {
            RunOutcome::Response(r) => format!(
                "→ {}  {DIM}{:.1} ms{DIM:#}",
                r.status,
                r.timings.total.as_secs_f64() * 1000.0
            ),
            RunOutcome::Invalid(_) => format!("{FAIL}not sent{FAIL:#}"),
            RunOutcome::Failed(_) => format!("{FAIL}failed{FAIL:#}"),
        };
        let attempts = if line.attempts > 1 {
            format!("  {DIM}{} attempts{DIM:#}", line.attempts)
        } else {
            String::new()
        };
        writeln!(
            out,
            "{mark} {:>2}  {:<width$}  {} {result}{attempts}",
            line.number,
            line.title,
            request_line(line.run)
        )?;
        details(out, line.run, verbose)?;
    }

    let (total, skipped) = counts(it);
    let ok = lines.iter().filter(|l| l.run.passed()).count();
    let noun = if info.kind == "flow" {
        "steps"
    } else {
        "requests"
    };
    let what = if info.kind == "flow" { "flow" } else { "run" };
    if it.passed() {
        writeln!(out, "{PASS}{what} passed: {ok} of {total} {noun}{PASS:#}")
    } else {
        let skipped = if skipped > 0 {
            format!(", {skipped} skipped")
        } else {
            String::new()
        };
        let hint = if verbose < 2
            && lines
                .iter()
                .any(|l| !l.run.passed() && l.run.response().is_some())
        {
            "  (-vv shows the trace of failures)"
        } else {
            ""
        };
        writeln!(
            out,
            "{FAIL}{what} failed: {ok} of {total} {noun} passed{skipped}{FAIL:#}{DIM}{hint}{DIM:#}"
        )
    }
}

/// Failing checks and missing captures (everything with `-v`), and the trace with `-vv`.
fn details(out: &mut dyn Write, run: &RequestRun, verbose: u8) -> io::Result<()> {
    let secrets = secrets_of(run);
    let passed = run.passed();
    match &run.outcome {
        RunOutcome::Invalid(why) => writeln!(out, "      {FAIL}{why}{FAIL:#}")?,
        RunOutcome::Failed(e) => {
            writeln!(out, "      {FAIL}{e}{FAIL:#}")?;
            if verbose >= 2
                && let Some(trace) = e.trace()
            {
                trace_view::render(&mut Indented(out), trace, verbose)?;
            }
        }
        RunOutcome::Response(resp) => {
            let mut checks: Vec<_> = run.assertions.iter().chain(&run.until).cloned().collect();
            assert_view::scrub(&mut checks, &secrets);
            let shown: Vec<_> = checks
                .into_iter()
                .filter(|c| verbose > 0 || !c.passed)
                .collect();
            if !shown.is_empty() {
                let mut buf = Vec::new();
                assert_view::render(&mut buf, &shown)?;
                // Without the summary line; the run has its own.
                for line in String::from_utf8_lossy(&buf).lines().take(shown.len()) {
                    writeln!(out, "      {line}")?;
                }
            }
            let captures: Vec<_> = run
                .captures
                .iter()
                .filter(|c| verbose > 0 || c.value.is_none())
                .cloned()
                .collect();
            if !captures.is_empty() {
                let mut buf = Vec::new();
                assert_view::render_captures(&mut buf, &captures, &secrets)?;
                for line in String::from_utf8_lossy(&buf).lines() {
                    writeln!(out, "      {line}")?;
                }
            }
            if !passed && verbose >= 2 {
                trace_view::render(&mut Indented(out), &resp.trace, verbose)?;
            }
        }
    }
    Ok(())
}

/// Indents every line written through it (trace output under a step).
struct Indented<'a>(&'a mut dyn Write);

impl Write for Indented<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let text = String::from_utf8_lossy(buf);
        let indented = text.replace('\n', "\n      ");
        self.0.write_all(indented.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush()
    }
}

fn step_json(line: &Line<'_>) -> Value {
    let run = line.run;
    let secrets = secrets_of(run);
    let mut assertions = run.assertions.clone();
    let mut until = run.until.clone();
    assert_view::scrub(&mut assertions, &secrets);
    assert_view::scrub(&mut until, &secrets);
    let (response, error) = match &run.outcome {
        RunOutcome::Response(r) => (
            json!({
                "status": r.status,
                "duration_ms": (r.timings.total.as_secs_f64() * 1_000_000.0).round() / 1000.0,
            }),
            Value::Null,
        ),
        RunOutcome::Invalid(why) => (
            Value::Null,
            json!({ "kind": "invalid_request", "message": why }),
        ),
        RunOutcome::Failed(e) => (
            Value::Null,
            json!({ "kind": "execution", "message": e.to_string() }),
        ),
    };
    json!({
        "number": line.number,
        "title": line.title,
        "key": run.key,
        "passed": run.passed(),
        "attempts": line.attempts,
        "request": run.request.as_ref().map(|r| {
            let url = r.effective_url().map_or_else(|_| r.url.clone(), |u| u.to_string());
            json!({ "method": r.method.as_str(), "url": json_out::scrub(r, &url) })
        }),
        "response": response,
        "error": error,
        "assertions": assertions,
        "until": until,
        "captures": assert_view::captures_json(&run.captures, &secrets),
    })
}

/// `--format json`.
fn json_doc(info: &Info, iterations: &[Iteration], planned: usize, declared: &Scopes) -> Value {
    let its: Vec<Value> = iterations
        .iter()
        .map(|it| {
            let (total, skipped) = counts(it);
            let data = it.data.as_ref().map(|row| {
                Value::Object(
                    row.0
                        .iter()
                        .map(|v| {
                            let value = if declared.is_declared_secret(&v.name) {
                                "<redacted>".to_owned()
                            } else {
                                v.value.clone().unwrap_or_default()
                            };
                            (v.name.clone(), Value::String(value))
                        })
                        .collect(),
                )
            });
            json!({
                "number": it.number,
                "data": data,
                "passed": it.passed(),
                "total": total,
                "skipped": skipped,
                "steps": lines(it).iter().map(step_json).collect::<Vec<_>>(),
            })
        })
        .collect();
    json!({
        "format": 1,
        "target": { "kind": info.kind, "key": info.key, "name": info.name },
        "passed": iterations.len() == planned && iterations.iter().all(Iteration::passed),
        "planned_iterations": planned,
        "iterations": its,
    })
}
