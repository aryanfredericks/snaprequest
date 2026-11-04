//! Hurl files (<https://hurl.dev>), imported as one flow: each entry becomes an inline step with
//! its captures and asserts. Queries, predicates and options SnapRequest has no equivalent for
//! are reported as warnings.

use serde_json::{Number, Value};
use snapreq_collection::assert::JsonType;
use snapreq_collection::vars::valid_name;
use snapreq_collection::{
    AuthSpec, BodySpec, CaptureSpec, Captures, Check, Checks, FlowFile, PartSpec, RetrySpec,
    StepSpec, Variables,
};
use snapreq_core::model::{HttpVersionPref, KeyValue, ProxyMode};

use crate::error::{Error, Result};
use crate::model::{Imported, Node};
use crate::util::{protect_headers, text_body, tidy_headers};

const FORMAT: &str = "Hurl";
const METHODS: [&str; 9] = [
    "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE", "CONNECT",
];

/// Whether the first meaningful line is a Hurl request line (`GET https://…`).
pub(crate) fn looks_like(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .is_some_and(|l| request_line(l).is_some())
}

fn request_line(line: &str) -> Option<(&str, &str)> {
    let (method, url) = line.split_once(char::is_whitespace)?;
    let url = url.trim();
    (METHODS.contains(&method) && !url.is_empty() && !url.contains(' ')).then_some((method, url))
}

/// Removes a trailing `# comment` outside double quotes and backticks.
fn strip_comment(line: &str) -> &str {
    let mut quoted = false;
    let mut prev = ' ';
    let mut escaped = false;
    for (i, c) in line.char_indices() {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == '"' || c == '`' {
            quoted = !quoted;
        } else if c == '#' && !quoted && prev.is_whitespace() {
            return line[..i].trim_end();
        }
        prev = c;
    }
    line.trim_end()
}

/// Hurl's built-in functions mapped to dynamic variables.
fn tpl(s: &str) -> String {
    s.replace("{{newUuid}}", "{{$uuid}}")
        .replace("{{newDate}}", "{{$isoTimestamp}}")
}

/// A value, unquoted when it is a `"…"` string.
fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 && s.starts_with('"') && s.ends_with('"') {
        let mut out = String::new();
        let mut chars = s[1..s.len() - 1].chars();
        while let Some(c) = chars.next() {
            if c == '\\' {
                match chars.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some(other) => out.push(other),
                    None => {}
                }
            } else {
                out.push(c);
            }
        }
        tpl(&out)
    } else {
        tpl(s)
    }
}

fn key_value(line: &str) -> Option<(String, String)> {
    let (k, v) = if let Some(rest) = line.strip_prefix('"') {
        let end = rest.find('"')?;
        let (k, after) = (&rest[..end], &rest[end + 1..]);
        (k, after.trim_start().strip_prefix(':')?)
    } else {
        line.split_once(':')?
    };
    Some((k.trim().to_owned(), unquote(v)))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    Headers,
    Query,
    Form,
    Multipart,
    Cookies,
    BasicAuth,
    Options,
    Captures,
    Asserts,
    ResponseHeaders,
    Unknown,
}

fn section(name: &str) -> Section {
    match name {
        "Query" | "QueryStringParams" => Section::Query,
        "Form" | "FormParams" => Section::Form,
        "Multipart" | "MultipartFormData" => Section::Multipart,
        "Cookies" => Section::Cookies,
        "BasicAuth" => Section::BasicAuth,
        "Options" => Section::Options,
        "Captures" => Section::Captures,
        "Asserts" => Section::Asserts,
        _ => Section::Unknown,
    }
}

pub(crate) fn import(text: &str) -> Result<Imported> {
    let mut imp = Imported::new("Hurl import");
    imp.reserve(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut steps = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let line = strip_comment(lines[i].trim());
        if line.is_empty() || line.starts_with('#') {
            i += 1;
            continue;
        }
        let Some((method, url)) = request_line(line) else {
            return Err(Error::invalid(
                FORMAT,
                format!(
                    "line {}: expected a request (`GET https://…`), found `{line}`",
                    i + 1
                ),
            ));
        };
        i += 1;
        let step = entry(&mut imp, method, url, &lines, &mut i)?;
        steps.push(step);
    }
    if steps.is_empty() {
        return Err(Error::invalid(FORMAT, "no request found"));
    }
    let flow = FlowFile {
        name: Some("Hurl import".into()),
        description: None,
        vars: Variables::default(),
        steps,
    };
    imp.items.push(Node::Flow {
        name: "flow".into(),
        file: flow,
    });
    Ok(imp)
}

/// One entry, after its request line; `i` ends at the next request line.
fn entry(
    imp: &mut Imported,
    method: &str,
    url: &str,
    lines: &[&str],
    i: &mut usize,
) -> Result<StepSpec> {
    let mut step = StepSpec {
        name: Some(crate::curl::title(method, url)),
        method: (method != "GET").then(|| method.to_owned()),
        url: Some(tpl(url)),
        ..StepSpec::default()
    };
    let mut cookies = Vec::new();
    let mut parts: Vec<PartSpec> = Vec::new();
    let mut form: Vec<KeyValue> = Vec::new();
    let mut sec = Section::Headers;
    let mut in_response = false;
    while *i < lines.len() {
        let raw = lines[*i];
        let line = strip_comment(raw.trim());
        if line.is_empty() || line.starts_with('#') {
            *i += 1;
            continue;
        }
        if request_line(line).is_some() {
            break;
        }
        *i += 1;
        if let Some(status) = line.strip_prefix("HTTP") {
            in_response = true;
            sec = Section::ResponseHeaders;
            let code = status.split_whitespace().last().unwrap_or_default();
            if let Ok(n) = code.parse::<u16>() {
                step.assert.status = Some(Check::equals(n));
            }
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            sec = section(&line[1..line.len() - 1]);
            if sec == Section::Unknown {
                imp.warn(format!("Hurl section `{line}` is not supported"));
            }
            continue;
        }
        if let Some(body) = body(imp, line, lines, i)? {
            if in_response {
                expect_body(&mut step, body);
            } else {
                step.body = Some(body);
            }
            continue;
        }
        match sec {
            Section::Headers => {
                if let Some((k, v)) = line.split_once(':') {
                    step.headers.0.push(KeyValue::new(k.trim(), tpl(v.trim())));
                }
            }
            Section::ResponseHeaders => {
                if let Some((k, v)) = key_value(line) {
                    step.assert.headers.0.push((k, Check::equals(v)));
                }
            }
            Section::Query => {
                if let Some((k, v)) = key_value(line) {
                    step.query.0.push(KeyValue::new(k, v));
                }
            }
            Section::Form => {
                if let Some((k, v)) = key_value(line) {
                    form.push(KeyValue::new(k, v));
                }
            }
            Section::Multipart => {
                if let Some((k, v)) = key_value(line) {
                    parts.push(multipart(imp, k, &v));
                }
            }
            Section::Cookies => {
                if let Some((k, v)) = key_value(line) {
                    cookies.push(format!("{k}={v}"));
                }
            }
            Section::BasicAuth => {
                if let Some((user, password)) = key_value(line) {
                    let password = imp.secret("password", &password);
                    step.auth = Some(AuthSpec::Basic {
                        username: user,
                        password,
                    });
                }
            }
            Section::Options => {
                if let Some((k, v)) = key_value(line) {
                    option(imp, &mut step, &k, &v);
                }
            }
            Section::Captures => capture(imp, &mut step.capture, line),
            Section::Asserts => assertion(imp, &mut step, line),
            Section::Unknown => {}
        }
    }
    if !cookies.is_empty() {
        step.headers
            .0
            .push(KeyValue::new("Cookie", cookies.join("; ")));
    }
    if !form.is_empty() {
        step.body = Some(BodySpec::Form {
            fields: form.into(),
        });
    }
    if !parts.is_empty() {
        step.body = Some(BodySpec::Multipart { parts });
    }
    protect_headers(imp, &mut step.headers, &mut step.auth);
    tidy_headers(&mut step.headers, step.body.as_ref());
    Ok(step)
}

fn multipart(imp: &mut Imported, name: String, v: &str) -> PartSpec {
    let mut part = PartSpec {
        name,
        value: None,
        file: None,
        filename: None,
        content_type: None,
        enabled: true,
    };
    if let Some(spec) = v.strip_prefix("file,") {
        let (path, ct) = spec.split_once(';').unwrap_or((spec, ""));
        part.file = Some(path.trim().into());
        let ct = ct.trim();
        if !ct.is_empty() {
            part.content_type = Some(ct.to_owned());
        }
        imp.warn(format!(
            "the file part `{}` refers to `{}`, relative to the flow file",
            part.name,
            path.trim()
        ));
    } else {
        part.value = Some(v.to_owned());
    }
    part
}

/// A body starting at `line` (which `i` is already past), or `None` when `line` is no body.
fn body(imp: &mut Imported, line: &str, lines: &[&str], i: &mut usize) -> Result<Option<BodySpec>> {
    if line.starts_with("```") {
        let lang = line.trim_start_matches('`').trim();
        let mut content = Vec::new();
        loop {
            let Some(l) = lines.get(*i) else {
                return Err(Error::invalid(FORMAT, "unterminated ``` body"));
            };
            *i += 1;
            if l.trim() == "```" {
                break;
            }
            content.push(*l);
        }
        let text = tpl(&content.join("\n"));
        return Ok(Some(match lang {
            "json" => BodySpec::Json {
                content: Some(text),
                file: None,
            },
            "graphql" => BodySpec::Json {
                content: Some(serde_json::json!({ "query": text }).to_string()),
                file: None,
            },
            "xml" => text_body(Some("application/xml"), &text),
            _ => text_body(None, &text).text_only(),
        }));
    }
    if line.starts_with('`') && line.ends_with('`') && line.len() >= 2 {
        return Ok(Some(
            text_body(None, &tpl(&line[1..line.len() - 1])).text_only(),
        ));
    }
    if line.starts_with('{') || line.starts_with('[') && !line.ends_with(']') || line == "[]" {
        // JSON, until the brackets balance.
        let mut text = String::from(line);
        let mut depth = json_depth(line);
        while depth > 0 {
            let Some(l) = lines.get(*i) else {
                return Err(Error::invalid(FORMAT, "unterminated JSON body"));
            };
            *i += 1;
            text.push('\n');
            text.push_str(l);
            depth += json_depth(l);
        }
        return Ok(Some(BodySpec::Json {
            content: Some(tpl(&text)),
            file: None,
        }));
    }
    if line.starts_with('<') {
        let mut text = String::from(line);
        while let Some(l) = lines.get(*i) {
            if l.trim().is_empty() || l.trim_start().starts_with("HTTP") {
                break;
            }
            *i += 1;
            text.push('\n');
            text.push_str(l);
        }
        return Ok(Some(text_body(Some("application/xml"), &tpl(&text))));
    }
    if let Some(spec) = line.strip_prefix("file,") {
        let path = spec.trim_end_matches(';').trim();
        imp.warn(format!(
            "the body file `{path}` is relative to the flow file"
        ));
        return Ok(Some(BodySpec::File {
            file: path.into(),
            content_type: None,
        }));
    }
    if line.starts_with("base64,") || line.starts_with("hex,") {
        imp.warn("base64 and hex bodies are not imported");
        return Ok(Some(BodySpec::Text {
            content: Some(String::new()),
            file: None,
            content_type: None,
        }));
    }
    Ok(None)
}

/// The change in bracket depth over a line of JSON (strings skipped).
fn json_depth(line: &str) -> i64 {
    let mut depth = 0;
    let mut in_str = false;
    let mut escaped = false;
    for c in line.chars() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' | '[' => depth += 1,
            '}' | ']' => depth -= 1,
            _ => {}
        }
    }
    depth
}

trait TextOnly {
    fn text_only(self) -> Self;
}

impl TextOnly for BodySpec {
    /// Raw Hurl text bodies are sent as text even when they look like JSON.
    fn text_only(self) -> Self {
        match self {
            Self::Json { content, file } => Self::Text {
                content,
                file,
                content_type: None,
            },
            other => other,
        }
    }
}

/// An expected response body: JSON compares as JSON, anything else as text.
fn expect_body(step: &mut StepSpec, body: BodySpec) {
    match body {
        BodySpec::Json {
            content: Some(text),
            ..
        } => match serde_json::from_str::<Value>(&text) {
            Ok(v) => step.assert.json.0.push(("$".into(), Check::equals(v))),
            Err(_) => step.assert.body = Some(Check::equals(text)),
        },
        BodySpec::Text {
            content: Some(text),
            ..
        } => step.assert.body = Some(Check::equals(text)),
        _ => {}
    }
}

fn option(imp: &mut Imported, step: &mut StepSpec, key: &str, v: &str) {
    let flag = v == "true";
    let settings = &mut step.settings;
    match key {
        "location" => settings.follow_redirects = Some(flag),
        "insecure" => settings.verify_tls = Some(!flag),
        "max-redirs" => settings.max_redirects = v.parse().ok(),
        "http1.0" | "http1.1" if flag => settings.http_version = Some(HttpVersionPref::Http1),
        "http2" | "http3" if flag => settings.http_version = Some(HttpVersionPref::Http2),
        "compressed" | "verbose" | "very-verbose" | "output" | "skip" | "path-as-is" => {}
        "proxy" => settings.proxy = Some(ProxyMode::Url(v.to_owned())),
        "cacert" => settings.ca_cert = Some(v.into()),
        "cert" => settings.client_cert = Some(v.into()),
        "key" => settings.client_key = Some(v.into()),
        "connect-timeout" => settings.connect_timeout_ms = millis(v),
        "max-time" => settings.timeout_ms = millis(v),
        "delay" => step.delay_ms = millis(v),
        "retry" => {
            let n: i64 = v.parse().unwrap_or(0);
            let max = if n < 0 {
                imp.warn("an unlimited Hurl retry became 60 attempts");
                60
            } else {
                u32::try_from(n).unwrap_or(u32::MAX - 1).saturating_add(1)
            };
            let every_ms = step.retry.as_ref().map_or(1000, |r| r.every_ms);
            step.retry = Some(RetrySpec {
                max,
                every_ms,
                until: None,
            });
        }
        "retry-interval" => {
            let every = millis(v).unwrap_or(1000);
            match &mut step.retry {
                Some(r) => r.every_ms = every,
                None => {
                    step.retry = Some(RetrySpec {
                        max: 1,
                        every_ms: every,
                        until: None,
                    });
                }
            }
        }
        "variable" => {
            if let Some((name, value)) = v.split_once('=')
                && valid_name(name.trim())
            {
                step.vars.set(name.trim(), value.trim());
            }
        }
        "user" => {
            let (user, password) = v.split_once(':').unwrap_or((v, ""));
            let password = imp.secret("password", password);
            step.auth = Some(AuthSpec::Basic {
                username: user.to_owned(),
                password,
            });
        }
        other => imp.warn(format!("Hurl option `{other}` is not supported")),
    }
}

/// `500`, `500ms`, `2s`, `1m` → milliseconds (Hurl's default unit is milliseconds, except for
/// `connect-timeout` and `max-time` given in seconds by older versions; numbers are read as ms).
fn millis(v: &str) -> Option<u64> {
    let v = v.trim();
    let (num, mult) = if let Some(n) = v.strip_suffix("ms") {
        (n, 1)
    } else if let Some(n) = v.strip_suffix('s') {
        (n, 1000)
    } else if let Some(n) = v.strip_suffix('m') {
        (n, 60_000)
    } else {
        (v, 1)
    };
    num.trim().parse::<u64>().ok().map(|n| n * mult)
}

/// Splits a query line into tokens: words, `"strings"` and `/regexes/`.
fn tokens(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
            continue;
        }
        let mut t = String::new();
        if c == '"' || c == '/' {
            let close = c;
            t.push(chars.next().unwrap_or(close));
            let mut escaped = false;
            for ch in chars.by_ref() {
                t.push(ch);
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == close {
                    break;
                }
            }
        } else {
            while let Some(&ch) = chars.peek() {
                if ch.is_whitespace() {
                    break;
                }
                t.push(ch);
                chars.next();
            }
        }
        out.push(t);
    }
    out
}

/// A quoted string or `/regex/` token's text.
fn text_arg(t: &str) -> String {
    if t.len() >= 2 && t.starts_with('/') && t.ends_with('/') {
        t[1..t.len() - 1].replace("\\/", "/")
    } else {
        unquote(t)
    }
}

/// The subject of a query: what it reads and how many tokens it used.
enum Subject {
    Status,
    Header(String),
    Cookie(String),
    Body,
    Json(String),
    Regex(String),
    Duration,
}

fn subject(t: &[String]) -> Option<(Subject, usize)> {
    let arg = || t.get(1).map(|a| text_arg(a));
    Some(match t.first()?.as_str() {
        "status" => (Subject::Status, 1),
        "body" => (Subject::Body, 1),
        "duration" => (Subject::Duration, 1),
        "header" => (Subject::Header(arg()?), 2),
        "cookie" => (Subject::Cookie(arg()?), 2),
        "jsonpath" => (Subject::Json(arg()?), 2),
        "regex" => (Subject::Regex(arg()?), 2),
        _ => return None,
    })
}

fn capture(imp: &mut Imported, captures: &mut Captures, line: &str) {
    let Some((name, query)) = line.split_once(':') else {
        return;
    };
    let name = name.trim().to_owned();
    let mut t = tokens(query);
    let secret = t.last().is_some_and(|l| l == "redact");
    if secret {
        t.pop();
    }
    let parsed = subject(&t).filter(|(_, used)| *used == t.len());
    let Some((subj, _)) = parsed else {
        imp.warn(format!(
            "the Hurl capture `{name}: {}` is not supported",
            query.trim()
        ));
        return;
    };
    let mut spec = CaptureSpec {
        secret,
        ..CaptureSpec::default()
    };
    match subj {
        Subject::Status => spec.status = Some(true),
        Subject::Body => spec.body = Some(true),
        Subject::Header(h) => spec.header = Some(h),
        Subject::Cookie(c) => spec.cookie = Some(c),
        Subject::Json(p) => spec.json = Some(p),
        Subject::Regex(r) => spec.regex = Some(r),
        Subject::Duration => {
            imp.warn(format!(
                "the Hurl capture `{name}: duration` is not supported"
            ));
            return;
        }
    }
    if valid_name(&name) {
        captures.0.push((name, spec));
    } else {
        imp.warn(format!(
            "the capture name `{name}` is not a valid variable name"
        ));
    }
}

/// A predicate value: a number, `"string"`, `true`/`false`/`null`, or a `/regex/`.
fn value(t: &str) -> Value {
    match t {
        "true" => Value::Bool(true),
        "false" => Value::Bool(false),
        "null" => Value::Null,
        _ => {
            if let Ok(n) = t.parse::<i64>() {
                Value::Number(n.into())
            } else if let Some(n) = t.parse::<f64>().ok().and_then(Number::from_f64) {
                Value::Number(n)
            } else {
                Value::String(text_arg(t))
            }
        }
    }
}

fn number(v: &Value) -> Option<Number> {
    match v {
        Value::Number(n) => Some(n.clone()),
        _ => None,
    }
}

fn assertion(imp: &mut Imported, step: &mut StepSpec, line: &str) {
    let t = tokens(line);
    let unsupported = |imp: &mut Imported| {
        imp.warn(format!("the Hurl assert `{line}` is not supported"));
    };
    let Some((subj, mut used)) = subject(&t) else {
        return unsupported(imp);
    };
    let mut count = false;
    if t.get(used).is_some_and(|f| f == "count") {
        count = true;
        used += 1;
    }
    let mut negate = false;
    if t.get(used).is_some_and(|w| w == "not") {
        negate = true;
        used += 1;
    }
    let Some(pred) = t.get(used).cloned() else {
        return unsupported(imp);
    };
    let arg = t.get(used + 1).map(|a| value(a));
    if t.len() > used + 2 {
        return unsupported(imp);
    }
    let mut check = Check::default();
    let ok = match (pred.as_str(), negate, arg) {
        ("==", false, Some(v)) if count => match v.as_u64() {
            Some(n) => {
                check.length = Some(n);
                true
            }
            None => false,
        },
        (_, _, _) if count => false,
        ("==", false, Some(v)) => {
            check.equals = Some(v);
            true
        }
        ("!=", false, Some(v)) | ("==", true, Some(v)) => {
            check.not_equals = Some(v);
            true
        }
        (">", false, Some(v)) => number(&v).map(|n| check.gt = Some(n)).is_some(),
        (">=", false, Some(v)) => number(&v).map(|n| check.gte = Some(n)).is_some(),
        ("<", false, Some(v)) => number(&v).map(|n| check.lt = Some(n)).is_some(),
        ("<=", false, Some(v)) => number(&v).map(|n| check.lte = Some(n)).is_some(),
        ("startsWith", false, Some(Value::String(s))) => {
            check.starts_with = Some(s);
            true
        }
        ("endsWith", false, Some(Value::String(s))) => {
            check.ends_with = Some(s);
            true
        }
        ("contains" | "includes", false, Some(v)) => {
            check.contains = Some(v);
            true
        }
        ("contains" | "includes", true, Some(v)) => {
            check.not_contains = Some(v);
            true
        }
        ("matches", false, Some(Value::String(s))) => {
            check.matches = Some(s);
            true
        }
        ("exists", n, None) => {
            check.exists = Some(!n);
            true
        }
        ("isEmpty", false, None) => {
            check.length = Some(0);
            true
        }
        (ty, false, None) => match ty {
            "isInteger" => Some(JsonType::Integer),
            "isFloat" | "isNumber" => Some(JsonType::Number),
            "isBoolean" => Some(JsonType::Boolean),
            "isString" => Some(JsonType::String),
            "isCollection" | "isList" => Some(JsonType::Array),
            "isObject" => Some(JsonType::Object),
            _ => None,
        }
        .map(|t| check.type_ = Some(t))
        .is_some(),
        _ => false,
    };
    if !ok {
        return unsupported(imp);
    }
    let a = &mut *step.assert;
    let slot = match subj {
        Subject::Status => &mut a.status,
        Subject::Body => &mut a.body,
        Subject::Duration => &mut a.duration_ms,
        Subject::Header(h) => return merge_into(imp, &mut a.headers, h, check, line),
        Subject::Json(p) => return merge_into(imp, &mut a.json, p, check, line),
        Subject::Cookie(_) | Subject::Regex(_) => return unsupported(imp),
    };
    match slot {
        Some(existing) => {
            if !merge(existing, check) {
                imp.warn(format!(
                    "the Hurl assert `{line}` repeats a check; it was skipped"
                ));
            }
        }
        None => *slot = Some(check),
    }
}

fn merge_into(imp: &mut Imported, checks: &mut Checks, key: String, check: Check, line: &str) {
    match checks.0.iter_mut().find(|(k, _)| *k == key) {
        Some((_, existing)) => {
            if !merge(existing, check) {
                imp.warn(format!(
                    "the Hurl assert `{line}` repeats a check; it was skipped"
                ));
            }
        }
        None => checks.0.push((key, check)),
    }
}

/// Adds the operators of `new` to `into`; `false` if one is already set.
fn merge(into: &mut Check, new: Check) -> bool {
    macro_rules! fields {
        ($($f:ident),*) => {{
            $(if new.$f.is_some() && into.$f.is_some() { return false; })*
            $(if new.$f.is_some() { into.$f = new.$f; })*
        }};
    }
    fields!(
        equals,
        not_equals,
        contains,
        not_contains,
        starts_with,
        ends_with,
        matches,
        exists,
        type_,
        length,
        lt,
        lte,
        gt,
        gte,
        one_of
    );
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_and_tokens() {
        assert_eq!(strip_comment("HTTP 200 # ok"), "HTTP 200");
        assert_eq!(strip_comment("x: \"a # b\""), "x: \"a # b\"");
        assert_eq!(
            tokens(r#"jsonpath "$.a b" matches /^\d+\/x$/"#),
            ["jsonpath", "\"$.a b\"", "matches", r"/^\d+\/x$/"]
        );
        assert_eq!(millis("2s"), Some(2000));
        assert_eq!(millis("250"), Some(250));
    }
}
