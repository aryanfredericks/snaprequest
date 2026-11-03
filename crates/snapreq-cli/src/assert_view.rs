//! Printing assertion outcomes.

use std::io::{self, Write};

use anstyle::{AnsiColor, Style};
use snapreq_test::{Captured, Outcome};

const PASS: Style = AnsiColor::Green.on_default();
const FAIL: Style = AnsiColor::Red.on_default().bold();
const DIM: Style = Style::new().dimmed();

/// Hides `secrets` in what the checks report (actual values can echo credentials).
pub(crate) fn scrub(outcomes: &mut [Outcome], secrets: &[String]) {
    let mut secrets: Vec<&String> = secrets.iter().filter(|s| !s.is_empty()).collect();
    secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    let hide = |s: &mut String| {
        for secret in &secrets {
            if s.contains(secret.as_str()) {
                *s = s.replace(secret.as_str(), "<redacted>");
            }
        }
    };
    for o in outcomes {
        hide(&mut o.check);
        if let Some(a) = &mut o.actual {
            hide(a);
        }
        if let Some(m) = &mut o.message {
            hide(m);
        }
    }
}

/// One line per check, then a summary:
///
/// ```text
/// ✓ status equals 201
/// ✗ json $.id type integer  (actual: "abc")
/// assertions: 1 passed, 1 failed
/// ```
pub(crate) fn render(out: &mut dyn Write, outcomes: &[Outcome]) -> io::Result<()> {
    for o in outcomes {
        if o.passed {
            writeln!(out, "{PASS}✓{PASS:#} {} {}", o.subject, o.check)?;
        } else {
            let detail = match (&o.message, &o.actual) {
                (Some(m), _) => m.clone(),
                (None, Some(a)) => format!("actual: {a}"),
                (None, None) => String::new(),
            };
            let detail = if detail.is_empty() {
                String::new()
            } else {
                format!("  {DIM}({detail}){DIM:#}")
            };
            writeln!(out, "{FAIL}✗ {} {}{FAIL:#}{detail}", o.subject, o.check)?;
        }
    }
    let failed = outcomes.iter().filter(|o| !o.passed).count();
    let passed = outcomes.len() - failed;
    if failed == 0 {
        writeln!(out, "{PASS}assertions: {passed} passed{PASS:#}")
    } else {
        writeln!(
            out,
            "{FAIL}assertions: {passed} passed, {failed} failed{FAIL:#}"
        )
    }
}

/// The value of a capture as shown: secret captures and known secrets hidden.
fn shown(c: &Captured, secrets: &[String]) -> Option<String> {
    let mut v = c.shown()?.to_owned();
    for s in secrets.iter().filter(|s| !s.is_empty()) {
        v = v.replace(s.as_str(), "<redacted>");
    }
    Some(v)
}

/// `captured name = value` lines (with `-v`).
pub(crate) fn render_captures(
    out: &mut dyn Write,
    captures: &[Captured],
    secrets: &[String],
) -> io::Result<()> {
    for c in captures {
        match (shown(c, secrets), &c.error) {
            (Some(v), _) => writeln!(out, "{DIM}captured{DIM:#} {} = {v}", c.name)?,
            (None, Some(e)) => writeln!(out, "{FAIL}not captured{FAIL:#} {}: {e}", c.name)?,
            (None, None) => {}
        }
    }
    Ok(())
}

/// `[{ "name", "value", "secret", "error"? }]` for `--format json`.
pub(crate) fn captures_json(captures: &[Captured], secrets: &[String]) -> serde_json::Value {
    serde_json::Value::Array(
        captures
            .iter()
            .map(|c| {
                let mut v = serde_json::json!({
                    "name": c.name,
                    "value": shown(c, secrets),
                    "secret": c.secret,
                });
                if let Some(e) = &c.error {
                    v["error"] = serde_json::json!(e);
                }
                v
            })
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(passed: bool, actual: Option<&str>, message: Option<&str>) -> Outcome {
        Outcome {
            subject: "json $.token".into(),
            check: "equals \"abc\"".into(),
            passed,
            actual: actual.map(str::to_owned),
            message: message.map(str::to_owned),
        }
    }

    #[test]
    fn lines_and_summary() {
        let mut outcomes = vec![
            outcome(true, Some("\"abc\""), None),
            outcome(false, Some("\"s3cret-token\""), None),
            outcome(false, Some("missing"), Some("the body is not JSON")),
        ];
        scrub(&mut outcomes, &["s3cret-token".into()]);
        let mut buf = Vec::new();
        render(&mut anstream::StripStream::new(&mut buf), &outcomes).unwrap();
        assert_eq!(
            String::from_utf8(buf).unwrap(),
            "✓ json $.token equals \"abc\"\n\
             ✗ json $.token equals \"abc\"  (actual: \"<redacted>\")\n\
             ✗ json $.token equals \"abc\"  (the body is not JSON)\n\
             assertions: 1 passed, 2 failed\n"
        );
    }
}
