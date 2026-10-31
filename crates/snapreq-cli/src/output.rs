//! Human-readable output: status line, headers and formatted bodies.

use std::fmt::Write as _;
use std::io::{self, Write};

use anstyle::{AnsiColor, Style};
use snapreq_core::model::{Body, Header, TraceEvent};
use snapreq_core::{Request, Response};

/// Which parts of the exchange to print (HTTPie-style letters).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Parts {
    /// `H`: request line and headers (as sent, redacted).
    pub request_headers: bool,
    /// `B`: request body.
    pub request_body: bool,
    /// `h`: status line and response headers.
    pub response_headers: bool,
    /// `b`: response body.
    pub response_body: bool,
    /// `m`: timing and size summary.
    pub meta: bool,
    /// `t`: timing waterfall.
    pub waterfall: bool,
}

impl Parts {
    /// Parses a `--print` value such as `hb` or `HBhbm`.
    pub(crate) fn parse(spec: &str) -> Result<Self, String> {
        let mut p = Self::default();
        for c in spec.chars() {
            match c {
                'H' => p.request_headers = true,
                'B' => p.request_body = true,
                'h' => p.response_headers = true,
                'b' => p.response_body = true,
                'm' => p.meta = true,
                't' => p.waterfall = true,
                other => {
                    return Err(format!(
                        "invalid --print letter `{other}` (use H, B, h, b, m, t)"
                    ));
                }
            }
        }
        Ok(p)
    }

    /// Default: everything useful on a terminal, only the body when piped.
    pub(crate) fn default_for(terminal: bool) -> Self {
        Self {
            response_headers: terminal,
            response_body: true,
            ..Self::default()
        }
    }
}

const NAME: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Cyan)));
const DIM: Style = Style::new().dimmed();
const BOLD: Style = Style::new().bold();
const KEY: Style = Style::new()
    .fg_color(Some(anstyle::Color::Ansi(AnsiColor::Blue)))
    .bold();
const STRING: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Green)));
const NUMBER: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Yellow)));
const LITERAL: Style = Style::new().fg_color(Some(anstyle::Color::Ansi(AnsiColor::Magenta)));

fn status_style(status: u16) -> Style {
    let color = match status {
        200..=299 => AnsiColor::Green,
        300..=399 => AnsiColor::Cyan,
        400..=499 => AnsiColor::Yellow,
        _ => AnsiColor::Red,
    };
    Style::new()
        .fg_color(Some(anstyle::Color::Ansi(color)))
        .bold()
}

/// Prints the selected parts. `out` strips colors when they are not wanted.
pub(crate) fn print(
    out: &mut dyn Write,
    req: &Request,
    resp: &Response,
    parts: Parts,
    terminal: bool,
) -> io::Result<()> {
    let mut sections = 0;
    let mut separate = |out: &mut dyn Write| -> io::Result<()> {
        if sections > 0 {
            writeln!(out)?;
        }
        sections += 1;
        Ok(())
    };

    if parts.request_headers
        && let Some((method, url, version, headers)) = last_request_headers(resp)
    {
        separate(out)?;
        writeln!(out, "{BOLD}{method} {url}{BOLD:#} {DIM}{version}{DIM:#}")?;
        write_headers(out, &headers)?;
    }
    if parts.request_body && !req.body.is_none() {
        separate(out)?;
        write_request_body(out, &req.body)?;
    }
    if parts.response_headers {
        separate(out)?;
        let style = status_style(resp.status);
        writeln!(
            out,
            "{DIM}{}{DIM:#} {style}{} {}{style:#}",
            resp.version,
            resp.status,
            resp.status_text().unwrap_or("")
        )?;
        write_headers(out, &resp.headers)?;
    }
    if parts.response_body && !resp.body.is_empty() {
        separate(out)?;
        write_body(out, &resp.body, resp.header("content-type"), terminal)?;
    }
    if parts.meta {
        separate(out)?;
        write_meta(out, resp)?;
    }
    if parts.waterfall {
        separate(out)?;
        crate::waterfall::render(
            out,
            &resp.trace,
            crate::waterfall::terminal_width(crate::waterfall::Stream::Stdout),
        )?;
    }
    out.flush()
}

/// The request line and headers of the final hop, as recorded (and redacted) in the trace.
fn last_request_headers(resp: &Response) -> Option<(String, String, String, Vec<Header>)> {
    resp.trace.iter().rev().find_map(|e| match &e.event {
        TraceEvent::RequestHeaders {
            method,
            url,
            version,
            headers,
        } => Some((
            method.clone(),
            url.clone(),
            version.to_string(),
            headers.clone(),
        )),
        _ => None,
    })
}

fn write_headers(out: &mut dyn Write, headers: &[Header]) -> io::Result<()> {
    for h in headers {
        writeln!(out, "{NAME}{}{NAME:#}: {}", h.name, h.value)?;
    }
    Ok(())
}

fn write_request_body(out: &mut dyn Write, body: &Body) -> io::Result<()> {
    match body {
        Body::None => Ok(()),
        Body::Json { content } => {
            write_body(out, content.as_bytes(), Some("application/json"), true)
        }
        Body::Text {
            content,
            content_type,
        } => write_body(out, content.as_bytes(), content_type.as_deref(), true),
        Body::Form { fields } => {
            for f in fields.iter().filter(|f| f.enabled) {
                writeln!(out, "{NAME}{}{NAME:#}={}", f.name, f.value)?;
            }
            Ok(())
        }
        Body::Multipart { parts } => {
            for p in parts.iter().filter(|p| p.enabled) {
                match &p.value {
                    snapreq_core::model::PartValue::Text { value } => {
                        writeln!(out, "{NAME}{}{NAME:#}={value}", p.name)?;
                    }
                    snapreq_core::model::PartValue::File { path, .. } => {
                        writeln!(out, "{NAME}{}{NAME:#}=@{}", p.name, path.display())?;
                    }
                }
            }
            Ok(())
        }
        Body::File { path, .. } => writeln!(out, "{DIM}[file body: {}]{DIM:#}", path.display()),
    }
}

/// Writes a body: JSON is pretty-printed and colored; binary data is summarized on a
/// terminal (it would garble it) and written raw otherwise.
pub(crate) fn write_body(
    out: &mut dyn Write,
    body: &[u8],
    content_type: Option<&str>,
    terminal: bool,
) -> io::Result<()> {
    let ct = content_type.unwrap_or_default().to_ascii_lowercase();
    let text = std::str::from_utf8(body).ok();

    let Some(text) = text else {
        if terminal {
            return writeln!(
                out,
                "{DIM}[binary body: {}{}; use -o FILE to save it]{DIM:#}",
                human_bytes(body.len() as u64),
                content_type.map(|c| format!(", {c}")).unwrap_or_default()
            );
        }
        return out.write_all(body);
    };

    if is_json(&ct, text)
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(text)
    {
        let mut s = String::with_capacity(text.len() * 2);
        json(&mut s, &value, 0);
        return writeln!(out, "{s}");
    }
    out.write_all(text.as_bytes())?;
    if !text.ends_with('\n') {
        writeln!(out)?;
    }
    Ok(())
}

fn is_json(content_type: &str, text: &str) -> bool {
    let mime = content_type.split(';').next().unwrap_or_default().trim();
    mime == "application/json"
        || mime.ends_with("+json")
        || mime == "text/json"
        // Many APIs send JSON as text/plain or without a type.
        || ((mime.is_empty() || mime == "text/plain")
            && matches!(text.trim_start().chars().next(), Some('{' | '[')))
}

/// Pretty-prints JSON with two-space indentation and colors.
fn json(out: &mut String, v: &serde_json::Value, indent: usize) {
    use serde_json::Value;
    let pad = |n: usize| "  ".repeat(n);
    match v {
        Value::Null => {
            let _ = write!(out, "{LITERAL}null{LITERAL:#}");
        }
        Value::Bool(b) => {
            let _ = write!(out, "{LITERAL}{b}{LITERAL:#}");
        }
        Value::Number(n) => {
            let _ = write!(out, "{NUMBER}{n}{NUMBER:#}");
        }
        Value::String(s) => {
            let _ = write!(out, "{STRING}{}{STRING:#}", Value::String(s.clone()));
        }
        Value::Array(items) if items.is_empty() => out.push_str("[]"),
        Value::Object(map) if map.is_empty() => out.push_str("{}"),
        Value::Array(items) => {
            out.push_str("[\n");
            for (i, item) in items.iter().enumerate() {
                out.push_str(&pad(indent + 1));
                json(out, item, indent + 1);
                out.push_str(if i + 1 < items.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(indent));
            out.push(']');
        }
        Value::Object(map) => {
            out.push_str("{\n");
            for (i, (k, item)) in map.iter().enumerate() {
                out.push_str(&pad(indent + 1));
                let _ = write!(out, "{KEY}{}{KEY:#}: ", Value::String(k.clone()));
                json(out, item, indent + 1);
                out.push_str(if i + 1 < map.len() { ",\n" } else { "\n" });
            }
            out.push_str(&pad(indent));
            out.push('}');
        }
    }
}

fn write_meta(out: &mut dyn Write, resp: &Response) -> io::Result<()> {
    let ms = |d: Option<std::time::Duration>| {
        d.map_or_else(
            || "-".to_owned(),
            |d| format!("{:.1} ms", d.as_secs_f64() * 1000.0),
        )
    };
    let t = &resp.timings;
    writeln!(
        out,
        "{DIM}total{DIM:#} {}  {DIM}dns{DIM:#} {}  {DIM}connect{DIM:#} {}  {DIM}tls{DIM:#} {}  {DIM}wait{DIM:#} {}  {DIM}download{DIM:#} {}",
        ms(Some(t.total)),
        ms(t.dns),
        ms(t.connect),
        ms(t.tls),
        ms(t.wait),
        ms(t.download)
    )?;
    writeln!(
        out,
        "{DIM}received{DIM:#} {} ({} decoded)  {DIM}sent{DIM:#} {}",
        human_bytes(resp.sizes.response_body + resp.sizes.response_headers),
        human_bytes(resp.sizes.response_body_decoded),
        human_bytes(resp.sizes.request_body + resp.sizes.request_headers),
    )
}

pub(crate) fn human_bytes(n: u64) -> String {
    const UNITS: [&str; 4] = ["KiB", "MiB", "GiB", "TiB"];
    if n < 1024 {
        return format!("{n} B");
    }
    let mut value = n as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Renders with colors stripped, as on a non-color terminal.
    fn plain(f: impl FnOnce(&mut dyn Write) -> io::Result<()>) -> String {
        let mut buf = Vec::new();
        {
            let mut out = anstream::StripStream::new(&mut buf);
            f(&mut out).unwrap();
        }
        String::from_utf8(buf).unwrap()
    }

    #[test]
    fn print_spec() {
        let p = Parts::parse("Hhbm").unwrap();
        assert!(p.request_headers && p.response_headers && p.response_body && p.meta);
        assert!(!p.request_body);
        assert!(Parts::parse("x").is_err());
        assert_eq!(
            Parts::default_for(false),
            Parts {
                response_body: true,
                ..Parts::default()
            }
        );
        assert!(Parts::default_for(true).response_headers);
    }

    #[test]
    fn json_is_pretty_printed_in_original_key_order() {
        let text = plain(|out| {
            write_body(
                out,
                br#"{"z":1,"a":[true,null,"x"],"e":{},"n":-1.5}"#,
                Some("application/json"),
                true,
            )
        });
        assert_eq!(
            text,
            "{\n  \"z\": 1,\n  \"a\": [\n    true,\n    null,\n    \"x\"\n  ],\n  \"e\": {},\n  \"n\": -1.5\n}\n"
        );
    }

    #[test]
    fn json_is_detected_without_a_json_content_type() {
        let text = plain(|out| write_body(out, br#"[1,2]"#, None, true));
        assert_eq!(text, "[\n  1,\n  2\n]\n");
        // Invalid JSON is printed as-is.
        let text = plain(|out| write_body(out, b"{not json", Some("application/json"), true));
        assert_eq!(text, "{not json\n");
    }

    #[test]
    fn strings_are_escaped() {
        let text =
            plain(|out| write_body(out, br#"{"q":"a \"b\"\n"}"#, Some("application/json"), true));
        assert_eq!(text, "{\n  \"q\": \"a \\\"b\\\"\\n\"\n}\n");
    }

    #[test]
    fn binary_bodies() {
        let data = [0xff, 0x00, 0x10];
        let text = plain(|out| write_body(out, &data, Some("image/png"), true));
        assert_eq!(
            text,
            "[binary body: 3 B, image/png; use -o FILE to save it]\n"
        );

        let mut raw = Vec::new();
        write_body(&mut raw, &data, Some("image/png"), false).unwrap();
        assert_eq!(raw, data);
    }

    #[test]
    fn sizes() {
        assert_eq!(human_bytes(999), "999 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(5 * 1024 * 1024), "5.0 MiB");
    }
}
