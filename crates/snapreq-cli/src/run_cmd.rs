//! `snapreq run`: a flow, a folder or a whole collection, optionally per data row.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anstyle::{AnsiColor, Style};
use snapreq_collection::{Collection, Environment, FLOW_SUFFIX, Scopes, Secrets};
use snapreq_core::{Error, Request};
use snapreq_test::report::{counts, lines, request_line, secrets_of};
use snapreq_test::{
    Iteration, Plan, Report, RequestRun, RunOptions, RunOutcome, Unit, data, execute, request_keys,
    resolve_unit,
};

use crate::cli::{ColorArg, FormatArg, ReportFormat, RunArgs};
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
        Target::Key(key) => match (c.find_flow(&key), folder) {
            (Some(_), Some(_)) => Err("--folder cannot be used with a flow".into()),
            _ => resolve_unit(c, Some(&key)),
        },
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

    let mut reports = Vec::new();
    for spec in &args.reports {
        match parse_report(spec) {
            Ok(r) => reports.push(r),
            Err(e) => return invalid(e),
        }
    }

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
        live: None,
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
    let secret_vars: Vec<String> = plan
        .overrides
        .0
        .iter()
        .chain(plan.rows.iter().flat_map(|r| &r.0))
        .map(|v| v.name.clone())
        .filter(|n| declared.is_declared_secret(n))
        .collect();
    let report = Report::new(&collection, &unit, &iterations, planned, secret_vars);

    let mut code = exit_code(&iterations);
    for (format, path) in &reports {
        let text = match format {
            ReportFormat::Junit => report.junit(),
            ReportFormat::Html => report.html(),
            ReportFormat::Json => {
                serde_json::to_string_pretty(&report.json(true)).unwrap_or_default() + "\n"
            }
        };
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            eprintln!("error: cannot create `{}`: {e}", dir.display());
            code = exit::INTERNAL;
            continue;
        }
        match std::fs::write(path, text) {
            Ok(()) if !json => eprintln!("report written to {}", path.display()),
            Ok(()) => {}
            Err(e) => {
                eprintln!("error: cannot write `{}`: {e}", path.display());
                code = exit::INTERNAL;
            }
        }
    }

    if json {
        crate::emit_json(&report.json(false));
    } else {
        let color = match args.color {
            ColorArg::Auto => anstream::ColorChoice::Auto,
            ColorArg::Always => anstream::ColorChoice::Always,
            ColorArg::Never => anstream::ColorChoice::Never,
        };
        let mut out = anstream::AutoStream::new(io::stdout().lock(), color);
        if let Err(e) = render(&mut out, &report, args.verbose)
            && e.kind() != io::ErrorKind::BrokenPipe
        {
            eprintln!("error: {e}");
            return exit::INTERNAL;
        }
    }
    code
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

fn render(out: &mut dyn Write, report: &Report<'_>, verbose: u8) -> io::Result<()> {
    let what = match report.kind {
        "collection" => "collection".to_owned(),
        _ => report.key.clone(),
    };
    writeln!(out, "{}  {DIM}{what}{DIM:#}", report.name)?;
    let (iterations, planned) = (report.iterations, report.planned);
    let many = planned > 1;
    for it in iterations {
        if many {
            let row = report
                .row(it)
                .map(|r| {
                    r.iter()
                        .map(|(k, v)| format!("{k}={v}"))
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .map(|r| format!("  {DIM}{r}{DIM:#}"))
                .unwrap_or_default();
            writeln!(out, "iteration {}/{planned}{row}", it.number)?;
        }
        render_iteration(out, report, it, verbose)?;
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
    info: &Report<'_>,
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

/// `junit:out.xml`, `html:report.html` or `json:run.json`.
fn parse_report(spec: &str) -> Result<(ReportFormat, PathBuf), String> {
    let (kind, path) = spec
        .split_once(':')
        .filter(|(_, p)| !p.is_empty())
        .ok_or_else(|| {
            format!("invalid --report `{spec}`, expected FORMAT:PATH (junit, html or json)")
        })?;
    let format = match kind {
        "junit" => ReportFormat::Junit,
        "html" => ReportFormat::Html,
        "json" => ReportFormat::Json,
        other => {
            return Err(format!(
                "unknown report format `{other}` (junit, html or json)"
            ));
        }
    };
    Ok((format, PathBuf::from(path)))
}
