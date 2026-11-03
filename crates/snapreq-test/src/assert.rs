//! Evaluating `[assert]` checks against a response.
//!
//! [`Assertions::compile`] validates everything that can be checked before sending
//! (JSONPath syntax, regular expressions, the schema file); [`Assertions::evaluate`] then
//! produces one [`Outcome`] per operator.

use std::path::{Path, PathBuf};

use regex::Regex;
use serde::Serialize;
use serde_json::Value;
use serde_json_path::JsonPath;
use snapreq_collection::{AssertSpec, Check, JsonType};
use snapreq_core::Response;

/// Longest rendering of an actual value in an outcome.
const MAX_ACTUAL: usize = 200;

/// The result of one check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Outcome {
    /// What was checked: `status`, `header content-type`, `json $.id`, `schema user.json`.
    pub subject: String,
    /// The check: `equals 201`, `type integer`, `< 800`, `valid`.
    pub check: String,
    pub passed: bool,
    /// The actual value (compact JSON, shortened), or `missing`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actual: Option<String>,
    /// Why the check could not be applied, or schema errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

enum Subject {
    Status,
    Duration,
    Body,
    Header(String),
    Json(String, JsonPath),
    Schema(PathBuf, Box<jsonschema::Validator>),
}

struct Item {
    subject: Subject,
    check: Check,
    regex: Option<Regex>,
}

/// Compiled checks of one request.
pub struct Assertions {
    items: Vec<Item>,
}

impl std::fmt::Debug for Assertions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Assertions")
            .field("checks", &self.items.len())
            .finish()
    }
}

impl Assertions {
    /// Checks the spec and prepares it. Errors name the check (`assert.json.$.x: ...`).
    pub fn compile(spec: &AssertSpec) -> Result<Self, String> {
        let mut items = Vec::new();
        let mut push = |subject: Subject, check: &Check, place: &str| -> Result<(), String> {
            let regex = check
                .matches
                .as_deref()
                .map(|r| {
                    Regex::new(r).map_err(|e| format!("{place}: invalid regular expression: {e}"))
                })
                .transpose()?;
            items.push(Item {
                subject,
                check: check.clone(),
                regex,
            });
            Ok(())
        };
        if let Some(c) = &spec.status {
            push(Subject::Status, c, "assert.status")?;
        }
        if let Some(c) = &spec.duration_ms {
            push(Subject::Duration, c, "assert.duration_ms")?;
        }
        if let Some(c) = &spec.body {
            push(Subject::Body, c, "assert.body")?;
        }
        for (name, c) in &spec.headers.0 {
            push(
                Subject::Header(name.trim().to_owned()),
                c,
                &format!("assert.headers.{name}"),
            )?;
        }
        for (path, c) in &spec.json.0 {
            let compiled = JsonPath::parse(path)
                .map_err(|e| format!("assert.json: invalid JSONPath `{path}`: {e}"))?;
            push(
                Subject::Json(path.clone(), compiled),
                c,
                &format!("assert.json.{path}"),
            )?;
        }
        if let Some(file) = &spec.schema {
            let validator = load_schema(file)?;
            items.push(Item {
                subject: Subject::Schema(file.clone(), Box::new(validator)),
                check: Check::default(),
                regex: None,
            });
        }
        Ok(Self { items })
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Runs every check against `resp`.
    pub fn evaluate(&self, resp: &Response) -> Vec<Outcome> {
        let body_json = std::cell::OnceCell::new();
        let json = || {
            body_json.get_or_init(|| {
                serde_json::from_slice::<Value>(&resp.body)
                    .map_err(|e| format!("the body is not JSON: {e}"))
            })
        };
        let mut out = Vec::new();
        for item in &self.items {
            let (subject, actual): (String, Result<Option<Value>, String>) = match &item.subject {
                Subject::Status => ("status".into(), Ok(Some(Value::from(resp.status)))),
                Subject::Duration => (
                    "duration_ms".into(),
                    Ok(serde_json::Number::from_f64(
                        (resp.timings.total.as_secs_f64() * 1_000_000.0).round() / 1000.0,
                    )
                    .map(Value::Number)),
                ),
                Subject::Body => (
                    "body".into(),
                    Ok(Some(Value::String(
                        String::from_utf8_lossy(&resp.body).into_owned(),
                    ))),
                ),
                Subject::Header(name) => {
                    let values: Vec<&str> = resp
                        .headers
                        .iter()
                        .filter(|h| h.name.eq_ignore_ascii_case(name))
                        .map(|h| h.value.as_str())
                        .collect();
                    (
                        format!("header {name}"),
                        Ok((!values.is_empty()).then(|| Value::String(values.join(", ")))),
                    )
                }
                Subject::Json(text, path) => (
                    format!("json {text}"),
                    json().clone().map(|doc| {
                        let nodes = path.query(&doc).all();
                        match nodes.as_slice() {
                            [] => None,
                            [one] => Some((*one).clone()),
                            many => Some(Value::Array(many.iter().map(|v| (*v).clone()).collect())),
                        }
                    }),
                ),
                Subject::Schema(file, validator) => {
                    out.push(schema_outcome(file, validator, json()));
                    continue;
                }
            };
            let is_status = matches!(item.subject, Subject::Status);
            match actual {
                Err(why) => out.extend(operators(&item.check).into_iter().map(|check| Outcome {
                    subject: subject.clone(),
                    check,
                    passed: false,
                    actual: None,
                    message: Some(why.clone()),
                })),
                Ok(actual) => out.extend(check_all(item, &subject, actual.as_ref(), is_status)),
            }
        }
        out
    }
}

fn load_schema(file: &Path) -> Result<jsonschema::Validator, String> {
    let text = std::fs::read_to_string(file)
        .map_err(|e| format!("assert.schema: cannot read `{}`: {e}", file.display()))?;
    let schema: Value = serde_json::from_str(&text)
        .map_err(|e| format!("assert.schema: `{}` is not JSON: {e}", file.display()))?;
    jsonschema::validator_for(&schema).map_err(|e| {
        format!(
            "assert.schema: `{}` is not a valid schema: {e}",
            file.display()
        )
    })
}

fn schema_outcome(
    file: &Path,
    validator: &jsonschema::Validator,
    body: &Result<Value, String>,
) -> Outcome {
    let name = file.file_name().map_or_else(
        || file.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let (passed, message) = match body {
        Err(why) => (false, Some(why.clone())),
        Ok(doc) => {
            let errors: Vec<String> = validator
                .iter_errors(doc)
                .take(5)
                .map(|e| {
                    let at = e.instance_path().to_string();
                    if at.is_empty() {
                        e.to_string()
                    } else {
                        format!("{at}: {e}")
                    }
                })
                .collect();
            if errors.is_empty() {
                (true, None)
            } else {
                (false, Some(errors.join("; ")))
            }
        }
    };
    Outcome {
        subject: format!("schema {name}"),
        check: "valid".into(),
        passed,
        actual: None,
        message,
    }
}

/// The operators set in `c`, rendered, in a fixed order.
fn operators(c: &Check) -> Vec<String> {
    let mut v = Vec::new();
    if let Some(e) = c.exists {
        v.push(if e {
            "exists".into()
        } else {
            "does not exist".into()
        });
    }
    if let Some(x) = &c.equals {
        v.push(format!("equals {}", render(x)));
    }
    if let Some(x) = &c.not_equals {
        v.push(format!("not equals {}", render(x)));
    }
    if let Some(x) = &c.one_of {
        v.push(format!("one of {}", render(&Value::Array(x.clone()))));
    }
    if let Some(x) = &c.contains {
        v.push(format!("contains {}", render(x)));
    }
    if let Some(x) = &c.not_contains {
        v.push(format!("not contains {}", render(x)));
    }
    if let Some(x) = &c.starts_with {
        v.push(format!("starts with {}", render(&Value::String(x.clone()))));
    }
    if let Some(x) = &c.ends_with {
        v.push(format!("ends with {}", render(&Value::String(x.clone()))));
    }
    if let Some(x) = &c.matches {
        v.push(format!("matches /{x}/"));
    }
    if let Some(t) = c.type_ {
        v.push(format!("type {}", t.as_str()));
    }
    if let Some(n) = c.length {
        v.push(format!("length {n}"));
    }
    for (op, n) in [("<", &c.lt), ("<=", &c.lte), (">", &c.gt), (">=", &c.gte)] {
        if let Some(n) = n {
            v.push(format!("{op} {n}"));
        }
    }
    v
}

fn check_all(item: &Item, subject: &str, actual: Option<&Value>, is_status: bool) -> Vec<Outcome> {
    let c = &item.check;
    let rendered = Some(actual.map_or_else(|| "missing".to_owned(), render));
    let mut out = Vec::new();
    let mut add = |check: String, result: Result<bool, String>| {
        let (passed, message) = match result {
            Ok(p) => (p, None),
            Err(m) => (false, Some(m)),
        };
        out.push(Outcome {
            subject: subject.to_owned(),
            check,
            passed,
            actual: rendered.clone(),
            message,
        });
    };
    let ops = operators(c);
    let mut names = ops.into_iter();
    let mut next = || names.next().unwrap_or_default();

    if let Some(e) = c.exists {
        add(next(), Ok(actual.is_some() == e));
    }

    if let Some(x) = &c.equals {
        add(
            next(),
            need(actual).map(|a| {
                if is_status {
                    status_matches(a, x)
                } else {
                    loose_eq(a, x)
                }
            }),
        );
    }
    if let Some(x) = &c.not_equals {
        add(next(), need(actual).map(|a| !loose_eq(a, x)));
    }
    if let Some(x) = &c.one_of {
        add(
            next(),
            need(actual).map(|a| {
                x.iter().any(|e| {
                    if is_status {
                        status_matches(a, e)
                    } else {
                        loose_eq(a, e)
                    }
                })
            }),
        );
    }
    if let Some(x) = &c.contains {
        add(next(), need(actual).and_then(|a| contains(a, x)));
    }
    if let Some(x) = &c.not_contains {
        add(
            next(),
            need(actual).and_then(|a| contains(a, x).map(|b| !b)),
        );
    }
    if let Some(x) = &c.starts_with {
        add(
            next(),
            need(actual).map(|a| text(a).starts_with(x.as_str())),
        );
    }
    if let Some(x) = &c.ends_with {
        add(next(), need(actual).map(|a| text(a).ends_with(x.as_str())));
    }
    if let Some(re) = &item.regex {
        add(next(), need(actual).map(|a| re.is_match(&text(a))));
    }
    if let Some(t) = c.type_ {
        add(next(), need(actual).map(|a| has_type(a, t)));
    }
    if let Some(n) = c.length {
        add(
            next(),
            need(actual).and_then(|a| match a {
                Value::String(s) => Ok(s.chars().count() as u64 == n),
                Value::Array(v) => Ok(v.len() as u64 == n),
                Value::Object(o) => Ok(o.len() as u64 == n),
                other => Err(format!("{} has no length", type_name(other))),
            }),
        );
    }
    for (bound, cmp) in [
        (&c.lt, (|a: f64, b: f64| a < b) as fn(f64, f64) -> bool),
        (&c.lte, |a, b| a <= b),
        (&c.gt, |a, b| a > b),
        (&c.gte, |a, b| a >= b),
    ] {
        if let Some(b) = bound {
            let b = b.as_f64().unwrap_or(f64::NAN);
            add(
                next(),
                need(actual).and_then(|a| {
                    number(a)
                        .map(|a| cmp(a, b))
                        .ok_or_else(|| format!("{} is not a number", type_name(a)))
                }),
            );
        }
    }
    out
}

/// Every operator except `exists` needs a value.
fn need(a: Option<&Value>) -> Result<&Value, String> {
    a.ok_or_else(|| "missing".to_owned())
}

/// Status checks accept classes such as `"2xx"`.
fn status_matches(actual: &Value, expected: &Value) -> bool {
    match expected {
        Value::String(s) if s.len() == 3 && s[1..].eq_ignore_ascii_case("xx") => {
            let class = s.as_bytes()[0];
            actual
                .as_u64()
                .is_some_and(|code| class.is_ascii_digit() && code / 100 == u64::from(class - b'0'))
        }
        // `status = [200, 201]` means one of them.
        Value::Array(options) => options.iter().any(|o| status_matches(actual, o)),
        other => loose_eq(actual, other),
    }
}

/// Equality that compares numbers by value and lets text (headers, the body) match
/// numbers and booleans written without quotes.
fn loose_eq(actual: &Value, expected: &Value) -> bool {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Value::String(a), Value::Number(b)) => a.trim().parse::<f64>().ok() == b.as_f64(),
        (Value::String(a), Value::Bool(b)) => a.trim() == b.to_string(),
        (Value::Array(a), Value::Array(b)) => {
            a.len() == b.len() && a.iter().zip(b).all(|(x, y)| loose_eq(x, y))
        }
        (Value::Object(a), Value::Object(b)) => {
            a.len() == b.len()
                && a.iter()
                    .all(|(k, v)| b.get(k).is_some_and(|w| loose_eq(v, w)))
        }
        (a, b) => a == b,
    }
}

fn contains(actual: &Value, needle: &Value) -> Result<bool, String> {
    match actual {
        Value::String(s) => Ok(s.contains(text(needle).as_str())),
        Value::Array(items) => Ok(items.iter().any(|i| loose_eq(i, needle))),
        Value::Object(o) => match needle {
            Value::String(key) => Ok(o.contains_key(key)),
            _ => Err("an object can only be checked for a key (a string)".into()),
        },
        other => Err(format!("cannot look inside {}", type_name(other))),
    }
}

/// A string as itself, anything else as JSON.
fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn number(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn has_type(v: &Value, t: JsonType) -> bool {
    match t {
        JsonType::String => v.is_string(),
        JsonType::Number => v.is_number(),
        JsonType::Integer => {
            v.is_i64() || v.is_u64() || v.as_f64().is_some_and(|f| f.fract() == 0.0)
        }
        JsonType::Boolean => v.is_boolean(),
        JsonType::Array => v.is_array(),
        JsonType::Object => v.is_object(),
        JsonType::Null => v.is_null(),
    }
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Compact JSON, shortened to [`MAX_ACTUAL`] characters.
fn render(v: &Value) -> String {
    let s = v.to_string();
    if s.chars().count() <= MAX_ACTUAL {
        return s;
    }
    let cut: String = s.chars().take(MAX_ACTUAL).collect();
    format!("{cut}…")
}
