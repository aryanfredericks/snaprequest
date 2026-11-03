//! `snapreq run`: running a flow.

use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anstyle::{AnsiColor, Style};
use serde_json::{Value, json};
use snapreq_collection::{Collection, Environment, FLOW_SUFFIX, Secrets};
use snapreq_core::{Error, Request};
use snapreq_test::{FlowRun, RunOutcome, Session, StepRun, run_flow};

use crate::cli::{ColorArg, FormatArg, RunArgs};
use crate::collection_cmd::{find_root, parse_vars};
use crate::send::ArgError;
use crate::{assert_view, exit, json_out, trace_view};

const PASS: Style = AnsiColor::Green.on_default();
const FAIL: Style = AnsiColor::Red.on_default().bold();
const DIM: Style = Style::new().dimmed();

/// The collection root and the flow key for `snapreq run TARGET`.
fn resolve(args: &RunArgs) -> Result<(PathBuf, String), ArgError> {
    if let Some(dir) = &args.collection {
        let root = find_root(dir)
            .ok_or_else(|| ArgError(format!("`{}` is not inside a collection", dir.display())))?;
        return Ok((root, args.target.clone()));
    }
    let path = Path::new(&args.target);
    if args.target.ends_with(FLOW_SUFFIX) && path.is_file() {
        let abs = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
        let root = abs
            .parent()
            .and_then(find_root)
            .ok_or_else(|| ArgError(format!("`{}` is not inside a collection", path.display())))?;
        let rel = abs.strip_prefix(&root).unwrap_or(&abs);
        let key = rel.to_string_lossy().replace('\\', "/");
        return Ok((root, key.trim_end_matches(FLOW_SUFFIX).to_owned()));
    }
    let root = find_root(Path::new(".")).ok_or_else(|| {
        ArgError(format!(
            "`{}` is not a flow file, and the current directory is not inside a collection (use -c DIR)",
            args.target
        ))
    })?;
    Ok((root, args.target.clone()))
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
    let (root, key) = match resolve(args) {
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
    let Some(flow) = collection.find_flow(&key) else {
        let known: Vec<&str> = collection.flows.iter().map(|f| f.key.as_str()).collect();
        return invalid(format!(
            "no flow `{key}` in the collection (flows: {})",
            if known.is_empty() {
                "none".into()
            } else {
                known.join(", ")
            }
        ));
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
    let timeout = match args.timeout {
        Some(t) if t.is_finite() && t >= 0.0 => (t > 0.0).then(|| Duration::from_secs_f64(t)),
        Some(_) => return invalid("--timeout must be a non-negative number of seconds".into()),
        None => None,
    };

    let mut session = Session::new(&collection, environment.as_ref(), &secrets, crate::client());
    let declared = session.scopes(&flow.key);
    for v in &overrides.0 {
        if let Some(value) = &v.value
            && declared.is_declared_secret(&v.name)
        {
            session.hide(value.clone());
        }
    }
    for s in &args.secrets {
        session.hide(s.clone());
    }
    session.overrides = overrides;

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
    let result = run_flow(&mut session, flow, adjust).await;

    if json {
        crate::emit_json(&json_doc(&result));
    } else {
        let color = match args.color {
            ColorArg::Auto => anstream::ColorChoice::Auto,
            ColorArg::Always => anstream::ColorChoice::Always,
            ColorArg::Never => anstream::ColorChoice::Never,
        };
        let mut out = anstream::AutoStream::new(io::stdout().lock(), color);
        if let Err(e) = render(&mut out, &result, args.verbose)
            && e.kind() != io::ErrorKind::BrokenPipe
        {
            eprintln!("error: {e}");
            return exit::INTERNAL;
        }
    }
    exit_code(&result)
}

fn exit_code(run: &FlowRun) -> u8 {
    if run.passed() {
        return exit::OK;
    }
    match run.steps.last().map(|s| &s.run.outcome) {
        Some(RunOutcome::Invalid(_)) => exit::INVALID_REQUEST,
        Some(RunOutcome::Failed(Error::Execution(e))) if e.timed_out => exit::TIMEOUT,
        Some(RunOutcome::Failed(_)) => exit::NETWORK,
        _ => exit::ASSERTION_FAILED,
    }
}

/// `METHOD url` with secrets removed, or the step title when nothing was built.
fn request_line(step: &StepRun) -> String {
    match &step.run.request {
        Some(req) => {
            let url = req
                .effective_url()
                .map_or_else(|_| req.url.clone(), |u| u.to_string());
            format!("{} {}", req.method, json_out::scrub(req, &url))
        }
        None => String::new(),
    }
}

fn secrets_of(step: &StepRun) -> Vec<String> {
    step.run
        .request
        .as_ref()
        .map(Request::secrets)
        .unwrap_or_default()
}

fn render(out: &mut dyn Write, run: &FlowRun, verbose: u8) -> io::Result<()> {
    writeln!(out, "{}  {DIM}{}{DIM:#}", run.name, run.key)?;
    let width = run
        .steps
        .iter()
        .map(|s| s.title.chars().count())
        .max()
        .unwrap_or(0);
    for step in &run.steps {
        let passed = step.run.passed();
        let mark = if passed {
            format!("{PASS}✓{PASS:#}")
        } else {
            format!("{FAIL}✗{FAIL:#}")
        };
        let result = match &step.run.outcome {
            RunOutcome::Response(r) => format!(
                "→ {}  {DIM}{:.1} ms{DIM:#}",
                r.status,
                r.timings.total.as_secs_f64() * 1000.0
            ),
            RunOutcome::Invalid(_) => format!("{FAIL}not sent{FAIL:#}"),
            RunOutcome::Failed(_) => format!("{FAIL}failed{FAIL:#}"),
        };
        let attempts = if step.attempts > 1 {
            format!("  {DIM}{} attempts{DIM:#}", step.attempts)
        } else {
            String::new()
        };
        writeln!(
            out,
            "{mark} {:>2}  {:<width$}  {} {result}{attempts}",
            step.number,
            step.title,
            request_line(step)
        )?;

        let secrets = secrets_of(step);
        match &step.run.outcome {
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
                let mut checks: Vec<_> = step
                    .run
                    .assertions
                    .iter()
                    .chain(&step.run.until)
                    .cloned()
                    .collect();
                assert_view::scrub(&mut checks, &secrets);
                let shown: Vec<_> = checks
                    .into_iter()
                    .filter(|c| verbose > 0 || !c.passed)
                    .collect();
                if !shown.is_empty() {
                    let mut buf = Vec::new();
                    assert_view::render(&mut buf, &shown)?;
                    let text = String::from_utf8_lossy(&buf);
                    // Without the summary line; the flow has its own.
                    for line in text.lines().take(shown.len()) {
                        writeln!(out, "      {line}")?;
                    }
                }
                let captures: Vec<_> = step
                    .run
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
    }
    let ok = run.steps.iter().filter(|s| s.run.passed()).count();
    if run.passed() {
        writeln!(
            out,
            "{PASS}flow passed: {ok} of {} steps{PASS:#}",
            run.total
        )
    } else {
        let skipped = if run.skipped() > 0 {
            format!(", {} skipped", run.skipped())
        } else {
            String::new()
        };
        let hint = if run.steps.last().is_some_and(|s| s.run.response().is_some()) {
            "  (-vv shows the failing step's trace)"
        } else {
            ""
        };
        writeln!(
            out,
            "{FAIL}flow failed: {ok} of {} steps passed{skipped}{FAIL:#}{DIM}{hint}{DIM:#}",
            run.total
        )
    }
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

/// `--format json`.
fn json_doc(run: &FlowRun) -> Value {
    let steps: Vec<Value> = run
        .steps
        .iter()
        .map(|step| {
            let secrets = secrets_of(step);
            let mut assertions = step.run.assertions.clone();
            let mut until = step.run.until.clone();
            assert_view::scrub(&mut assertions, &secrets);
            assert_view::scrub(&mut until, &secrets);
            let (response, error) = match &step.run.outcome {
                RunOutcome::Response(r) => (
                    json!({
                        "status": r.status,
                        "duration_ms": (r.timings.total.as_secs_f64() * 1_000_000.0).round() / 1000.0,
                    }),
                    Value::Null,
                ),
                RunOutcome::Invalid(why) => (Value::Null, json!({ "kind": "invalid_request", "message": why })),
                RunOutcome::Failed(e) => (Value::Null, json!({ "kind": "execution", "message": e.to_string() })),
            };
            json!({
                "number": step.number,
                "title": step.title,
                "passed": step.run.passed(),
                "attempts": step.attempts,
                "request": step.run.request.as_ref().map(|r| {
                    let url = r.effective_url().map_or_else(|_| r.url.clone(), |u| u.to_string());
                    json!({ "method": r.method.as_str(), "url": json_out::scrub(r, &url) })
                }),
                "response": response,
                "error": error,
                "assertions": assertions,
                "until": until,
                "captures": assert_view::captures_json(&step.run.captures, &secrets),
            })
        })
        .collect();
    json!({
        "format": 1,
        "flow": { "key": run.key, "name": run.name },
        "passed": run.passed(),
        "total": run.total,
        "skipped": run.skipped(),
        "steps": steps,
    })
}
