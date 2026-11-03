//! Reports of a run: the JSON document, JUnit XML and a single-file HTML page.
//!
//! Everything here hides the secret values known for each request (credentials, secret
//! variables, secret captures). Unlike the CLI's own output, reports also hide them in
//! response bodies, because reports are made to be shared.

use std::fmt::Write as _;

use serde_json::{Value, json};
use snapreq_collection::Collection;
use snapreq_core::Request;
use snapreq_core::model::{TraceEntry, TraceEvent};

use crate::assert::Outcome;
use crate::capture::Captured;
use crate::run::{Iteration, Unit, UnitRun};
use crate::session::{RequestRun, RunOutcome};

/// Longest response body shown in a report (bytes of text).
const MAX_BODY: usize = 64 * 1024;

/// Replaces every secret (longest first) with `<redacted>`, also in the escaped form a
/// secret takes inside JSON text (checks show actual values as JSON).
pub fn hide(text: &str, secrets: &[String]) -> String {
    let mut forms: Vec<String> = Vec::new();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        let escaped = serde_json::to_string(s).unwrap_or_default();
        let escaped = escaped
            .strip_prefix('"')
            .and_then(|e| e.strip_suffix('"'))
            .unwrap_or_default()
            .to_owned();
        for f in [s.clone(), escaped] {
            if !f.is_empty() && !forms.contains(&f) {
                forms.push(f);
            }
        }
    }
    forms.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let mut out = text.to_owned();
    for f in forms {
        if out.contains(f.as_str()) {
            out = out.replace(f.as_str(), "<redacted>");
        }
    }
    out
}

/// Hides secrets in what checks report.
pub fn scrub_outcomes(outcomes: &mut [Outcome], secrets: &[String]) {
    for o in outcomes {
        o.check = hide(&o.check, secrets);
        o.actual = o.actual.as_deref().map(|a| hide(a, secrets));
        o.message = o.message.as_deref().map(|m| hide(m, secrets));
    }
}

/// The value of a capture as shown: secret captures and known secrets hidden.
pub fn shown_capture(c: &Captured, secrets: &[String]) -> Option<String> {
    c.shown().map(|v| hide(v, secrets))
}

/// `[{ "name", "value", "secret", "error"? }]`.
pub fn captures_json(captures: &[Captured], secrets: &[String]) -> Value {
    Value::Array(
        captures
            .iter()
            .map(|c| {
                let mut v = json!({
                    "name": c.name,
                    "value": shown_capture(c, secrets),
                    "secret": c.secret,
                });
                if let Some(e) = &c.error {
                    v["error"] = json!(e);
                }
                v
            })
            .collect(),
    )
}

/// Every secret of a request: credentials, secret variables, earlier secret captures and
/// the values this request captured as secret (its own checks may report them).
pub fn secrets_of(run: &RequestRun) -> Vec<String> {
    let mut out = run
        .request
        .as_ref()
        .map(Request::secrets)
        .unwrap_or_default();
    out.extend(
        run.captures
            .iter()
            .filter(|c| c.secret)
            .filter_map(|c| c.value.clone()),
    );
    out
}

/// `value` as JSON with every secret hidden in its strings.
fn hidden_json(value: Value, secrets: &[String]) -> Value {
    match value {
        Value::String(s) => Value::String(hide(&s, secrets)),
        Value::Array(a) => Value::Array(a.into_iter().map(|v| hidden_json(v, secrets)).collect()),
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, v)| (k, hidden_json(v, secrets)))
                .collect(),
        ),
        other => other,
    }
}

/// `METHOD url` with secrets removed, or empty when nothing was built.
pub fn request_line(run: &RequestRun) -> String {
    match &run.request {
        Some(req) => {
            let url = req
                .effective_url()
                .map_or_else(|_| req.url.clone(), |u| u.to_string());
            format!("{} {}", req.method, hide_url(req, &url))
        }
        None => String::new(),
    }
}

/// Hides secrets in a URL, including their URL-encoded forms.
pub fn hide_url(req: &Request, url: &str) -> String {
    let mut secrets = req.secrets();
    let encoded: Vec<String> = secrets
        .iter()
        .map(|s| url::form_urlencoded::byte_serialize(s.as_bytes()).collect())
        .collect();
    secrets.extend(encoded);
    hide(url, &secrets)
}

/// One request or flow step of an iteration.
#[derive(Debug, Clone, Copy)]
pub struct Line<'r> {
    pub number: usize,
    pub title: &'r str,
    pub attempts: u32,
    pub run: &'r RequestRun,
}

/// The requests or steps of an iteration, in order.
pub fn lines(it: &Iteration) -> Vec<Line<'_>> {
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

/// Total and skipped requests or steps of an iteration.
pub fn counts(it: &Iteration) -> (usize, usize) {
    match &it.result {
        UnitRun::Flow(f) => (f.total, f.skipped()),
        UnitRun::Requests { runs, total } => (*total, total - runs.len()),
    }
}

fn duration_ms(run: &RequestRun) -> Option<f64> {
    run.response()
        .map(|r| (r.timings.total.as_secs_f64() * 1_000_000.0).round() / 1000.0)
}

/// A finished run, ready to be reported.
#[derive(Debug)]
pub struct Report<'a> {
    /// `flow`, `folder` or `collection`.
    pub kind: &'static str,
    /// The flow or folder key (empty for a whole collection).
    pub key: String,
    pub name: String,
    /// Titles of every step or request, including ones not run.
    pub titles: Vec<String>,
    pub iterations: &'a [Iteration],
    /// Iterations planned (more than run when the run bailed out).
    pub planned: usize,
    /// Variable names whose data-row values are hidden.
    pub secret_vars: Vec<String>,
}

impl<'a> Report<'a> {
    pub fn new(
        collection: &Collection,
        unit: &Unit<'_>,
        iterations: &'a [Iteration],
        planned: usize,
        secret_vars: Vec<String>,
    ) -> Self {
        let (kind, key, name, titles) = match unit {
            Unit::Flow(f) => (
                "flow",
                f.key.clone(),
                f.name().to_owned(),
                f.file.steps.iter().map(|s| s.title()).collect(),
            ),
            Unit::Requests(keys) => {
                let folder = if keys.len() == collection.requests().len() {
                    String::new()
                } else {
                    keys[0]
                        .rsplit_once('/')
                        .map(|(f, _)| f.to_owned())
                        .unwrap_or_default()
                };
                (
                    if folder.is_empty() {
                        "collection"
                    } else {
                        "folder"
                    },
                    folder,
                    collection.file.name.clone(),
                    keys.clone(),
                )
            }
        };
        Self {
            kind,
            key,
            name,
            titles,
            iterations,
            planned,
            secret_vars,
        }
    }

    pub fn passed(&self) -> bool {
        self.iterations.len() == self.planned && self.iterations.iter().all(Iteration::passed)
    }

    /// The data row of an iteration as `(name, value)` with secret values hidden.
    pub fn row(&self, it: &Iteration) -> Option<Vec<(String, String)>> {
        let row = it.data.as_ref()?;
        Some(
            row.0
                .iter()
                .map(|v| {
                    let value = if self.secret_vars.contains(&v.name) {
                        "<redacted>".to_owned()
                    } else {
                        v.value.clone().unwrap_or_default()
                    };
                    (v.name.clone(), value)
                })
                .collect(),
        )
    }

    /// The run as JSON (`snapreq run --format json`); with `details`, every response's
    /// headers, body and trace too.
    pub fn json(&self, details: bool) -> Value {
        let iterations: Vec<Value> = self
            .iterations
            .iter()
            .map(|it| {
                let (total, skipped) = counts(it);
                let data = self.row(it).map(|row| {
                    Value::Object(
                        row.into_iter()
                            .map(|(k, v)| (k, Value::String(v)))
                            .collect(),
                    )
                });
                json!({
                    "number": it.number,
                    "data": data,
                    "passed": it.passed(),
                    "total": total,
                    "skipped": skipped,
                    "steps": lines(it).iter().map(|l| step_json(l, details)).collect::<Vec<_>>(),
                })
            })
            .collect();
        json!({
            "format": 1,
            "target": { "kind": self.kind, "key": self.key, "name": self.name },
            "passed": self.passed(),
            "planned_iterations": self.planned,
            "iterations": iterations,
        })
    }

    fn suite_name(&self, it: &Iteration) -> String {
        let base = if self.key.is_empty() {
            self.name.clone()
        } else {
            format!("{} ({})", self.name, self.key)
        };
        if self.planned <= 1 {
            return base;
        }
        match self.row(it) {
            Some(row) => {
                let row: Vec<String> = row.iter().map(|(k, v)| format!("{k}={v}")).collect();
                format!("{base} [iteration {}: {}]", it.number, row.join(", "))
            }
            None => format!("{base} [iteration {}]", it.number),
        }
    }

    /// JUnit XML: one `<testsuite>` per iteration, one `<testcase>` per request or step.
    /// Failed checks are failures; requests that could not be built or sent are errors;
    /// requests not run are skipped.
    pub fn junit(&self) -> String {
        let mut suites = String::new();
        let (mut all_tests, mut all_fail, mut all_err, mut all_skip, mut all_time) =
            (0, 0, 0, 0, 0.0);
        for it in self.iterations {
            let classname = if self.key.is_empty() {
                &self.name
            } else {
                &self.key
            };
            let mut cases = String::new();
            let (mut fail, mut err, mut time) = (0, 0, 0.0);
            let lines = lines(it);
            for line in &lines {
                let secs = duration_ms(line.run).unwrap_or(0.0) / 1000.0;
                time += secs;
                let name = format!("{}. {}", line.number, line.title);
                let _ = write!(
                    cases,
                    "    <testcase classname=\"{}\" name=\"{}\" time=\"{secs:.3}\"",
                    xml(classname),
                    xml(&name)
                );
                let secrets = secrets_of(line.run);
                let body = match &line.run.outcome {
                    RunOutcome::Invalid(why) => {
                        err += 1;
                        format!(
                            "      <error type=\"invalid_request\" message=\"{}\"/>\n",
                            xml(&hide(why, &secrets))
                        )
                    }
                    RunOutcome::Failed(e) => {
                        err += 1;
                        format!(
                            "      <error type=\"execution\" message=\"{}\"/>\n",
                            xml(&hide(&e.to_string(), &secrets))
                        )
                    }
                    RunOutcome::Response(_) if line.run.passed() => String::new(),
                    RunOutcome::Response(_) => {
                        fail += 1;
                        let problems = problems(line.run, &secrets);
                        format!(
                            "      <failure type=\"assertion\" message=\"{}\">{}</failure>\n",
                            xml(problems.first().map_or("", String::as_str)),
                            xml(&problems.join("\n"))
                        )
                    }
                };
                let out = format!(
                    "{}{}",
                    request_line(line.run),
                    line.run
                        .response()
                        .map(|r| format!(" → {}", r.status))
                        .unwrap_or_default()
                );
                let attempts = if line.attempts > 1 {
                    format!(" ({} attempts)", line.attempts)
                } else {
                    String::new()
                };
                let _ = writeln!(
                    cases,
                    ">\n{body}      <system-out>{}{}</system-out>\n    </testcase>",
                    xml(&out),
                    xml(&attempts)
                );
            }
            let (total, skipped) = counts(it);
            for title in self.titles.iter().skip(lines.len()).take(skipped) {
                let n = cases.matches("<testcase").count() + 1;
                let _ = writeln!(
                    cases,
                    "    <testcase classname=\"{}\" name=\"{}\" time=\"0\">\n      <skipped/>\n    </testcase>",
                    xml(classname),
                    xml(&format!("{n}. {title}"))
                );
            }
            let _ = writeln!(
                suites,
                "  <testsuite name=\"{}\" tests=\"{total}\" failures=\"{fail}\" errors=\"{err}\" skipped=\"{skipped}\" time=\"{time:.3}\">\n{cases}  </testsuite>",
                xml(&self.suite_name(it))
            );
            all_tests += total;
            all_fail += fail;
            all_err += err;
            all_skip += skipped;
            all_time += time;
        }
        format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<testsuites name=\"{}\" tests=\"{all_tests}\" failures=\"{all_fail}\" errors=\"{all_err}\" skipped=\"{all_skip}\" time=\"{all_time:.3}\">\n{suites}</testsuites>\n",
            xml(&self.name)
        )
    }

    /// A self-contained HTML page (no external assets) with every request's checks,
    /// captures, headers, body and trace.
    pub fn html(&self) -> String {
        let mut body = String::new();
        let status = if self.passed() { "passed" } else { "failed" };
        let what = if self.key.is_empty() {
            self.kind.to_owned()
        } else {
            self.key.clone()
        };
        let _ = writeln!(
            body,
            "<header><h1>{}</h1><p class=\"muted\">{} · <span class=\"{status}\">{status}</span> · {} of {} iteration(s) run</p></header>",
            html(&self.name),
            html(&what),
            self.iterations.len(),
            self.planned
        );
        for it in self.iterations {
            let (total, skipped) = counts(it);
            let ok = lines(it).iter().filter(|l| l.run.passed()).count();
            let title = if self.planned > 1 {
                let row = self
                    .row(it)
                    .map(|r| {
                        r.iter()
                            .map(|(k, v)| format!("{k}={v}"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    })
                    .map(|r| format!(" <span class=\"muted\">{}</span>", html(&r)))
                    .unwrap_or_default();
                format!("<h2>Iteration {}{row}</h2>", it.number)
            } else {
                String::new()
            };
            let skipped_text = if skipped > 0 {
                format!(", {skipped} skipped")
            } else {
                String::new()
            };
            let _ = writeln!(
                body,
                "<section>{title}<p class=\"summary {}\">{ok} of {total} passed{skipped_text}</p>",
                if it.passed() { "passed" } else { "failed" }
            );
            let lines = lines(it);
            for line in &lines {
                body.push_str(&step_html(line));
            }
            for title in self.titles.iter().skip(lines.len()).take(skipped) {
                let _ = writeln!(
                    body,
                    "<div class=\"step skipped\"><span class=\"mark\">–</span> {} <span class=\"muted\">skipped</span></div>",
                    html(title)
                );
            }
            body.push_str("</section>\n");
        }
        format!(
            "<!DOCTYPE html>\n<html lang=\"en\"><head><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>{} · SnapRequest report</title><style>{CSS}</style></head><body>\n{body}<footer class=\"muted\">SnapRequest {}</footer></body></html>\n",
            html(&self.name),
            snapreq_core::VERSION
        )
    }
}

/// One line per failing check, missing capture or retry condition.
fn problems(run: &RequestRun, secrets: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    let mut checks: Vec<Outcome> = run
        .assertions
        .iter()
        .chain(&run.until)
        .filter(|o| !o.passed)
        .cloned()
        .collect();
    scrub_outcomes(&mut checks, secrets);
    for o in checks {
        let detail = o
            .message
            .clone()
            .or_else(|| o.actual.as_ref().map(|a| format!("actual: {a}")))
            .map(|d| format!(" ({d})"))
            .unwrap_or_default();
        out.push(format!("{} {}{detail}", o.subject, o.check));
    }
    for c in run.captures.iter().filter(|c| c.value.is_none()) {
        out.push(format!(
            "capture {}: {}",
            c.name,
            hide(c.error.as_deref().unwrap_or("missing"), secrets)
        ));
    }
    out
}

fn step_json(line: &Line<'_>, details: bool) -> Value {
    let run = line.run;
    let secrets = secrets_of(run);
    let mut assertions = run.assertions.clone();
    let mut until = run.until.clone();
    scrub_outcomes(&mut assertions, &secrets);
    scrub_outcomes(&mut until, &secrets);
    let (response, error) = match &run.outcome {
        RunOutcome::Response(r) => {
            let mut v = json!({ "status": r.status, "duration_ms": duration_ms(run) });
            if details {
                v["version"] = json!(r.version);
                v["headers"] = json!(r.headers);
                v["body"] = json!(body_text(&r.body, &secrets));
                v["timings"] = json!(r.timings);
                v["trace"] = hidden_json(json!(r.trace), &secrets);
            }
            (v, Value::Null)
        }
        RunOutcome::Invalid(why) => (
            Value::Null,
            json!({ "kind": "invalid_request", "message": hide(why, &secrets) }),
        ),
        RunOutcome::Failed(e) => {
            let mut v = json!({ "kind": "execution", "message": hide(&e.to_string(), &secrets) });
            if details && let Some(trace) = e.trace() {
                v["trace"] = hidden_json(json!(trace), &secrets);
            }
            (Value::Null, v)
        }
    };
    json!({
        "number": line.number,
        "title": line.title,
        "key": run.key,
        "passed": run.passed(),
        "attempts": line.attempts,
        "request": run.request.as_ref().map(|r| {
            let url = r.effective_url().map_or_else(|_| r.url.clone(), |u| u.to_string());
            json!({ "method": r.method.as_str(), "url": hide_url(r, &url) })
        }),
        "response": response,
        "error": error,
        "assertions": assertions,
        "until": until,
        "captures": captures_json(&run.captures, &secrets),
    })
}

/// The body as text (secrets hidden, shortened), or a note for binary bodies.
fn body_text(body: &[u8], secrets: &[String]) -> String {
    match std::str::from_utf8(body) {
        Ok(text) => {
            let mut t = hide(text, secrets);
            if t.len() > MAX_BODY {
                let mut cut = MAX_BODY;
                while !t.is_char_boundary(cut) {
                    cut -= 1;
                }
                t.truncate(cut);
                t.push_str("\n… (shortened)");
            }
            t
        }
        Err(_) => format!("<{} bytes of binary data>", body.len()),
    }
}

fn step_html(line: &Line<'_>) -> String {
    let run = line.run;
    let secrets = secrets_of(run);
    let passed = run.passed();
    let (status, time) = match &run.outcome {
        RunOutcome::Response(r) => (
            format!("→ {}", r.status),
            format!("{:.1} ms", duration_ms(run).unwrap_or(0.0)),
        ),
        RunOutcome::Invalid(_) => ("not sent".into(), String::new()),
        RunOutcome::Failed(_) => ("failed".into(), String::new()),
    };
    let attempts = if line.attempts > 1 {
        format!(" · {} attempts", line.attempts)
    } else {
        String::new()
    };
    let mut s = format!(
        "<details class=\"step {}\"{}><summary><span class=\"mark\">{}</span> <b>{}. {}</b> <code>{}</code> {} <span class=\"muted\">{time}{attempts}</span></summary>\n<div class=\"body\">\n",
        if passed { "passed" } else { "failed" },
        if passed { "" } else { " open" },
        if passed { "✓" } else { "✗" },
        line.number,
        html(line.title),
        html(&request_line(run)),
        html(&status)
    );
    match &run.outcome {
        RunOutcome::Invalid(why) => {
            let _ = writeln!(s, "<p class=\"failed\">{}</p>", html(&hide(why, &secrets)));
        }
        RunOutcome::Failed(e) => {
            let _ = writeln!(
                s,
                "<p class=\"failed\">{}</p>",
                html(&hide(&e.to_string(), &secrets))
            );
            if let Some(trace) = e.trace() {
                s.push_str(&trace_html(trace));
            }
        }
        RunOutcome::Response(resp) => {
            let mut checks: Vec<Outcome> =
                run.assertions.iter().chain(&run.until).cloned().collect();
            scrub_outcomes(&mut checks, &secrets);
            if !checks.is_empty() {
                s.push_str("<h4>Checks</h4><table>");
                for c in &checks {
                    let detail = c
                        .message
                        .clone()
                        .or_else(|| c.actual.clone())
                        .unwrap_or_default();
                    let _ = write!(
                        s,
                        "<tr class=\"{}\"><td>{}</td><td>{} {}</td><td class=\"muted\">{}</td></tr>",
                        if c.passed { "passed" } else { "failed" },
                        if c.passed { "✓" } else { "✗" },
                        html(&c.subject),
                        html(&c.check),
                        html(&detail)
                    );
                }
                s.push_str("</table>\n");
            }
            if !run.captures.is_empty() {
                s.push_str("<h4>Captures</h4><table>");
                for c in &run.captures {
                    let value = shown_capture(c, &secrets).unwrap_or_else(|| {
                        format!("not captured: {}", c.error.as_deref().unwrap_or("missing"))
                    });
                    let _ = write!(
                        s,
                        "<tr><td>{}</td><td><code>{}</code></td></tr>",
                        html(&c.name),
                        html(&value)
                    );
                }
                s.push_str("</table>\n");
            }
            s.push_str(&waterfall_html(resp));
            s.push_str("<h4>Response headers</h4><pre>");
            for h in &resp.headers {
                let _ = writeln!(s, "{}: {}", html(&h.name), html(&hide(&h.value, &secrets)));
            }
            let _ = writeln!(
                s,
                "</pre><h4>Response body</h4><pre>{}</pre>",
                html(&body_text(&resp.body, &secrets))
            );
            s.push_str(&trace_html(&resp.trace));
        }
    }
    s.push_str("</div></details>\n");
    s
}

/// Bars for the phases of the final exchange.
fn waterfall_html(resp: &snapreq_core::Response) -> String {
    let t = &resp.timings;
    let total = t.total.as_secs_f64().max(f64::EPSILON);
    let phases = [
        ("DNS", t.dns),
        ("connect", t.connect),
        ("TLS", t.tls),
        ("send", t.send),
        ("wait", t.wait),
        ("download", t.download),
    ];
    let mut s = String::from("<h4>Timing</h4><div class=\"waterfall\">");
    let mut start = total
        - phases
            .iter()
            .filter_map(|(_, d)| *d)
            .map(|d| d.as_secs_f64())
            .sum::<f64>();
    start = start.max(0.0);
    for (name, d) in phases {
        let Some(d) = d else { continue };
        let d = d.as_secs_f64();
        let _ = write!(
            s,
            "<div class=\"phase\"><span>{name}</span><span class=\"track\"><span class=\"bar {}\" style=\"margin-left:{:.2}%;width:{:.2}%\"></span></span><span class=\"muted\">{:.2} ms</span></div>",
            name.to_ascii_lowercase(),
            start / total * 100.0,
            (d / total * 100.0).max(0.5),
            d * 1000.0
        );
        start += d;
    }
    let _ = writeln!(
        s,
        "</div><p class=\"muted\">total {:.2} ms</p>",
        t.total.as_secs_f64() * 1000.0
    );
    s
}

/// The (already redacted) trace as readable lines.
fn trace_html(trace: &[TraceEntry]) -> String {
    let mut s = String::from("<details class=\"trace\"><summary>Trace</summary><pre>");
    for e in trace {
        let at = format!("[{:>9.3} ms] ", e.at.as_secs_f64() * 1000.0);
        let text = match &e.event {
            TraceEvent::DnsEnd { host, addrs, .. } => format!(
                "* resolved {host} to {}",
                addrs
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
            TraceEvent::ConnectEnd { addr, .. } => format!("* connected to {addr}"),
            TraceEvent::ConnectionReused { addr } => format!("* reusing connection to {addr}"),
            TraceEvent::TlsEnd { info, .. } => format!(
                "* {} {}{}",
                info.version,
                info.cipher,
                info.alpn
                    .as_deref()
                    .map(|a| format!(", ALPN {a}"))
                    .unwrap_or_default()
            ),
            TraceEvent::RequestHeaders {
                method,
                url,
                headers,
                ..
            } => {
                let mut t = format!("> {method} {url}");
                for h in headers {
                    let _ = write!(t, "\n> {}: {}", h.name, h.value);
                }
                t
            }
            TraceEvent::ResponseHeaders {
                status, headers, ..
            } => {
                let mut t = format!("< {status}");
                for h in headers {
                    let _ = write!(t, "\n< {}: {}", h.name, h.value);
                }
                t
            }
            TraceEvent::Redirect { status, to, .. } => format!("* {status} redirect to {to}"),
            TraceEvent::AuthChallenge { status, scheme } => {
                format!("* {status} {scheme} challenge; sending again with credentials")
            }
            TraceEvent::Info { message } => format!("* {message}"),
            TraceEvent::Error { phase, message } => format!("! {phase} failed: {message}"),
            _ => continue,
        };
        s.push_str(&html(&format!(
            "{at}{}",
            text.replace('\n', &format!("\n{}", " ".repeat(at.len())))
        )));
        s.push('\n');
    }
    s.push_str("</pre></details>\n");
    s
}

fn xml(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\n' | '\t' | '\r' => out.push(c),
            // Characters XML 1.0 does not allow.
            c if (c as u32) < 0x20 => out.push('\u{fffd}'),
            c => out.push(c),
        }
    }
    out
}

fn html(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
    out
}

const CSS: &str = "
:root{color-scheme:light dark;--bg:#fff;--fg:#1d1d1f;--muted:#6e6e73;--line:#e5e5ea;--ok:#1a7f37;--bad:#cf222e;--code:#f6f8fa}
@media (prefers-color-scheme:dark){:root{--bg:#111;--fg:#e6e6e6;--muted:#9a9a9f;--line:#2c2c2e;--ok:#3fb950;--bad:#f85149;--code:#1b1b1d}}
body{font:14px/1.5 system-ui,-apple-system,Segoe UI,sans-serif;background:var(--bg);color:var(--fg);max-width:1100px;margin:2rem auto;padding:0 1rem}
h1{margin:0}h2{font-size:1.1rem;margin:1.5rem 0 .5rem}h4{margin:1rem 0 .3rem;font-size:.85rem;text-transform:uppercase;letter-spacing:.04em;color:var(--muted)}
.muted{color:var(--muted)}.passed{color:var(--ok)}.failed{color:var(--bad)}
code,pre{font:12px/1.45 ui-monospace,SFMono-Regular,Menlo,Consolas,monospace}
pre{background:var(--code);padding:.6rem .8rem;border-radius:6px;overflow:auto;max-height:28rem;white-space:pre-wrap;word-break:break-all}
.step{border:1px solid var(--line);border-radius:8px;margin:.4rem 0;padding:.4rem .7rem}
.step summary{cursor:pointer;list-style:none}.step summary::-webkit-details-marker{display:none}
.step.passed .mark{color:var(--ok)}.step.failed .mark{color:var(--bad)}.step.failed{border-color:var(--bad)}
.step.skipped{color:var(--muted)}.body{padding:.3rem 0 .5rem}
table{border-collapse:collapse;width:100%}td{border-top:1px solid var(--line);padding:.25rem .4rem;vertical-align:top}
tr.failed td{color:var(--bad)}tr td:first-child{width:1.5rem}
.waterfall .phase{display:grid;grid-template-columns:5.5rem 1fr 6rem;gap:.5rem;align-items:center;font-size:12px}
.track{background:var(--code);height:.7rem;border-radius:3px;display:block}
.bar{display:block;height:100%;border-radius:3px;background:#8b949e}
.bar.dns{background:#0969da}.bar.connect{background:#bf8700}.bar.tls{background:#8250df}.bar.send{background:#1b7c83}.bar.wait{background:#1a7f37}.bar.download{background:#57ab5a}
.summary{font-weight:600}footer{margin-top:2rem;font-size:12px}
";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hide_raw_and_json_escaped_secrets() {
        let secrets = vec![
            "line1\nsay \"hi\"".to_owned(),
            String::new(),
            "tok".to_owned(),
        ];
        assert_eq!(
            hide(
                r#"raw: line1
say "hi" json: "line1\nsay \"hi\"" tok"#,
                &secrets
            ),
            r#"raw: <redacted> json: "<redacted>" <redacted>"#
        );
    }
}
