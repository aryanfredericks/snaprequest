//! `curl` commands, as copied from browsers, API docs and terminals.
//!
//! Shell quoting (`'…'`, `"…"`, `$'…'`), line continuations (`\`, and `^` from Windows `cmd`)
//! and several commands in one input (one per line, or joined with `;`, `&&`, `|`) are understood.
//! `{{name}}` in a command is kept as a placeholder.

use snapreq_collection::{AuthSpec, BodySpec, KeyValues, PartSpec, RequestFile};
use snapreq_core::model::{HttpVersionPref, KeyValue, ProxyMode};

use crate::error::{Error, Result};
use crate::model::{Imported, Node};
use crate::util::{form_encode, header, protect_headers, text_body, tidy_headers};

const FORMAT: &str = "cURL";

/// Whether the text starts with a `curl` command.
pub(crate) fn looks_like(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .is_some_and(|l| {
            let l = l.trim_start_matches("$ ").trim_start();
            l.starts_with("curl ") || l.starts_with("curl.exe ") || l.starts_with("curl\t")
        })
}

pub(crate) fn import(text: &str) -> Result<Imported> {
    let mut imported = Imported::new("Imported");
    imported.reserve(text);
    let commands = split_commands(&tokenize(text)?);
    for words in commands {
        let Some(first) = words.first() else { continue };
        let program = first.rsplit(['/', '\\']).next().unwrap_or(first);
        if program != "curl" && program != "curl.exe" {
            continue;
        }
        let request = command(&mut imported, &words[1..])?;
        imported.items.push(Node::Request(request));
    }
    if imported.items.is_empty() {
        return Err(Error::invalid(FORMAT, "no `curl` command found"));
    }
    // Named after the first request's host.
    if let Some(Node::Request(r)) = imported.items.first() {
        let rest = r
            .url
            .split_once("://")
            .map_or(r.url.as_str(), |(_, rest)| rest);
        let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
        if !host.is_empty() && !host.contains("{{") {
            imported.collection.name = host.to_owned();
        }
    }
    Ok(imported)
}

/// A word of a command line, or a separator between commands.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    Word(String),
    Separator,
}

/// Splits shell text into words, following POSIX quoting rules plus `$'…'`.
fn tokenize(text: &str) -> Result<Vec<Token>> {
    let mut tokens = Vec::new();
    let mut word = String::new();
    let mut in_word = false;
    let mut chars = text.chars().peekable();
    let unterminated = || Error::invalid(FORMAT, "unterminated quote");
    while let Some(c) = chars.next() {
        match c {
            '\\' => match chars.next() {
                Some('\n') => {}
                Some('\r') if chars.peek() == Some(&'\n') => {
                    chars.next();
                }
                Some(n) => {
                    word.push(n);
                    in_word = true;
                }
                None => {}
            },
            '^' if matches!(chars.peek(), Some('\n' | '\r')) => {
                // Windows cmd continuation.
                while matches!(chars.peek(), Some('\n' | '\r')) {
                    chars.next();
                }
            }
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => word.push(ch),
                        None => return Err(unterminated()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(ch @ ('"' | '\\' | '$' | '`')) => word.push(ch),
                            Some('\n') => {}
                            Some(ch) => {
                                word.push('\\');
                                word.push(ch);
                            }
                            None => return Err(unterminated()),
                        },
                        Some(ch) => word.push(ch),
                        None => return Err(unterminated()),
                    }
                }
            }
            '$' if chars.peek() == Some(&'\'') => {
                chars.next();
                in_word = true;
                ansi_c(&mut chars, &mut word).ok_or_else(unterminated)?;
            }
            '#' if !in_word => {
                for ch in chars.by_ref() {
                    if ch == '\n' {
                        tokens.push(Token::Separator);
                        break;
                    }
                }
            }
            '\n' | ';' | '|' | '&' => {
                if in_word {
                    tokens.push(Token::Word(std::mem::take(&mut word)));
                    in_word = false;
                }
                tokens.push(Token::Separator);
            }
            c if c.is_whitespace() => {
                if in_word {
                    tokens.push(Token::Word(std::mem::take(&mut word)));
                    in_word = false;
                }
            }
            c => {
                word.push(c);
                in_word = true;
            }
        }
    }
    if in_word {
        tokens.push(Token::Word(word));
    }
    Ok(tokens)
}

/// The body of `$'…'` (ANSI-C quoting); `None` when unterminated.
fn ansi_c(chars: &mut std::iter::Peekable<std::str::Chars<'_>>, word: &mut String) -> Option<()> {
    loop {
        match chars.next()? {
            '\'' => return Some(()),
            '\\' => {
                let e = chars.next()?;
                match e {
                    'n' => word.push('\n'),
                    't' => word.push('\t'),
                    'r' => word.push('\r'),
                    '0' => word.push('\0'),
                    'x' | 'u' | 'U' => {
                        let max = match e {
                            'x' => 2,
                            'u' => 4,
                            _ => 8,
                        };
                        let mut hex = String::new();
                        while hex.len() < max && chars.peek().is_some_and(char::is_ascii_hexdigit) {
                            hex.extend(chars.next());
                        }
                        let ch = u32::from_str_radix(&hex, 16)
                            .ok()
                            .and_then(char::from_u32)?;
                        word.push(ch);
                    }
                    other => word.push(other),
                }
            }
            c => word.push(c),
        }
    }
}

/// Groups words into commands; a leading `$` prompt is dropped.
fn split_commands(tokens: &[Token]) -> Vec<Vec<String>> {
    let mut out = Vec::new();
    let mut current: Vec<String> = Vec::new();
    for t in tokens {
        match t {
            Token::Word(w) => {
                if current.is_empty() && w == "$" {
                    continue;
                }
                current.push(w.clone());
            }
            Token::Separator => {
                if !current.is_empty() {
                    out.push(std::mem::take(&mut current));
                }
            }
        }
    }
    if !current.is_empty() {
        out.push(current);
    }
    out
}

/// Options taking a value, by long name; short forms map to these.
const WITH_VALUE: &[&str] = &[
    "request",
    "header",
    "data",
    "data-ascii",
    "data-binary",
    "data-raw",
    "data-urlencode",
    "json",
    "form",
    "form-string",
    "user",
    "user-agent",
    "referer",
    "cookie",
    "url",
    "max-time",
    "connect-timeout",
    "proxy",
    "cacert",
    "cert",
    "key",
    "max-redirs",
    "oauth2-bearer",
    "aws-sigv4",
    "noproxy",
    "proxy-user",
    // Accepted and ignored.
    "output",
    "write-out",
    "cookie-jar",
    "dump-header",
    "retry",
    "retry-delay",
    "retry-max-time",
    "resolve",
    "connect-to",
    "interface",
    "limit-rate",
    "config",
    "trace",
    "trace-ascii",
    "stderr",
    "upload-file",
    "range",
    "speed-time",
    "speed-limit",
    "cert-type",
    "key-type",
    "pass",
    "capath",
    "ciphers",
    "unix-socket",
    "abstract-unix-socket",
    "keepalive-time",
    "expect100-timeout",
    "happy-eyeballs-timeout-ms",
];

fn short(c: char) -> Option<&'static str> {
    Some(match c {
        'X' => "request",
        'H' => "header",
        'd' => "data",
        'F' => "form",
        'u' => "user",
        'A' => "user-agent",
        'e' => "referer",
        'b' => "cookie",
        'm' => "max-time",
        'x' => "proxy",
        'E' => "cert",
        'o' => "output",
        'w' => "write-out",
        'c' => "cookie-jar",
        'D' => "dump-header",
        'T' => "upload-file",
        'r' => "range",
        'K' => "config",
        'U' => "proxy-user",
        'y' => "speed-time",
        'Y' => "speed-limit",
        // Flags.
        'G' => "get",
        'I' => "head",
        'L' => "location",
        'k' => "insecure",
        's' => "silent",
        'S' => "show-error",
        'v' => "verbose",
        'i' => "include",
        'f' => "fail",
        'g' => "globoff",
        'N' => "no-buffer",
        'n' => "netrc",
        'O' => "remote-name",
        'J' => "remote-header-name",
        'Z' => "parallel",
        'q' => "disable",
        '#' => "progress-bar",
        '0' => "http1.0",
        '1' => "tlsv1",
        '2' => "sslv2",
        '3' => "sslv3",
        '4' => "ipv4",
        '6' => "ipv6",
        _ => return None,
    })
}

/// The parsed options of one command.
struct Parsed {
    method: Option<String>,
    urls: Vec<String>,
    headers: Vec<KeyValue>,
    data: Vec<String>,
    data_is_json: bool,
    form: Vec<PartSpec>,
    get: bool,
    head: bool,
    user: Option<String>,
    digest: bool,
    aws_sigv4: Option<String>,
    bearer: Option<String>,
    request: RequestFile,
}

fn command(imported: &mut Imported, args: &[String]) -> Result<RequestFile> {
    let mut p = Parsed {
        method: None,
        urls: Vec::new(),
        headers: Vec::new(),
        data: Vec::new(),
        data_is_json: false,
        form: Vec::new(),
        get: false,
        head: false,
        user: None,
        digest: false,
        aws_sigv4: None,
        bearer: None,
        request: RequestFile::new("GET", ""),
    };
    let mut i = 0;
    let mut options_done = false;
    while i < args.len() {
        let arg = &args[i];
        i += 1;
        if options_done || !arg.starts_with('-') || arg == "-" {
            p.urls.push(arg.clone());
            continue;
        }
        if arg == "--" {
            options_done = true;
            continue;
        }
        let take = |name: &str, i: &mut usize| -> Result<String> {
            let v = args.get(*i).cloned().ok_or_else(|| {
                Error::invalid(FORMAT, format!("option `--{name}` needs a value"))
            })?;
            *i += 1;
            Ok(v)
        };
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((n, v)) if WITH_VALUE.contains(&n) => (n, Some(v.to_owned())),
                _ => (long, None),
            };
            if WITH_VALUE.contains(&name) {
                let value = match inline {
                    Some(v) => v,
                    None => take(name, &mut i)?,
                };
                option(imported, &mut p, name, Some(value));
            } else {
                option(imported, &mut p, name, None);
            }
            continue;
        }
        // Short options, possibly combined (`-sSL`) or with an attached value (`-XPOST`).
        let shorts: Vec<char> = arg[1..].chars().collect();
        let mut j = 0;
        while j < shorts.len() {
            let c = shorts[j];
            j += 1;
            let Some(name) = short(c) else {
                imported.warn(format!("curl: ignored unknown option `-{c}`"));
                continue;
            };
            if WITH_VALUE.contains(&name) {
                let rest: String = shorts[j..].iter().collect();
                let value = if rest.is_empty() {
                    take(name, &mut i)?
                } else {
                    rest
                };
                option(imported, &mut p, name, Some(value));
                break;
            }
            option(imported, &mut p, name, None);
        }
    }
    finish(imported, p)
}

fn option(imported: &mut Imported, p: &mut Parsed, name: &str, value: Option<String>) {
    let v = value.unwrap_or_default();
    let settings = &mut p.request.settings;
    match name {
        "request" => p.method = Some(v.to_ascii_uppercase()),
        "header" => {
            if let Some((n, val)) = v.split_once(':') {
                let val = val.trim();
                // `Name:` removes a header in curl; there is nothing to remove here.
                if !val.is_empty() {
                    p.headers.push(KeyValue::new(n.trim(), val));
                }
            } else if let Some(n) = v.strip_suffix(';') {
                p.headers.push(KeyValue::new(n.trim(), ""));
            } else if v.starts_with('@') {
                imported.warn("curl: headers read from files (`-H @file`) are not imported");
            }
        }
        "data" | "data-ascii" | "data-binary" | "data-raw" => {
            if name != "data-raw" && v.starts_with('@') {
                imported.warn(format!(
                    "curl: `--{name} {v}` reads a file; the request body is left empty"
                ));
                p.data.push(String::new());
            } else if name == "data-binary" {
                p.data.push(v);
            } else {
                // curl strips newlines from `-d` data.
                p.data.push(v.replace(['\r', '\n'], ""));
            }
        }
        "data-urlencode" => {
            let encoded = match v.split_once('=') {
                Some(("", val)) => form_encode(val),
                Some((n, val)) => format!("{n}={}", form_encode(val)),
                None => form_encode(&v),
            };
            p.data.push(encoded);
        }
        "json" => {
            p.data.push(v);
            p.data_is_json = true;
        }
        "form" | "form-string" => p.form.push(form_part(imported, &v, name == "form")),
        "get" => p.get = true,
        "head" => p.head = true,
        "user" => p.user = Some(v),
        "digest" => p.digest = true,
        "basic" => p.digest = false,
        "aws-sigv4" => p.aws_sigv4 = Some(v),
        "oauth2-bearer" => p.bearer = Some(v),
        "user-agent" => p.headers.push(KeyValue::new("User-Agent", v)),
        "referer" => p.headers.push(KeyValue::new("Referer", v)),
        "cookie" => {
            if v.contains('=') {
                p.headers.push(KeyValue::new("Cookie", v));
            } else {
                imported.warn("curl: cookie files (`-b file`) are not imported");
            }
        }
        "url" => p.urls.push(v),
        "location" => settings.follow_redirects = Some(true),
        "max-redirs" => settings.max_redirects = v.parse().ok(),
        "insecure" => settings.verify_tls = Some(false),
        "max-time" => settings.timeout_ms = seconds_ms(&v),
        "connect-timeout" => settings.connect_timeout_ms = seconds_ms(&v),
        "proxy" => settings.proxy = Some(ProxyMode::Url(v)),
        "noproxy" if v == "*" => settings.proxy = Some(ProxyMode::Direct),
        "http1.0" | "http1.1" => settings.http_version = Some(HttpVersionPref::Http1),
        "http2" | "http2-prior-knowledge" | "http3" | "http3-only" => {
            settings.http_version = Some(HttpVersionPref::Http2);
        }
        "cacert" => settings.ca_cert = Some(v.into()),
        "cert" => {
            // `file:password`: the password is not kept.
            let file = v.split_once(':').map_or(v.as_str(), |(f, _)| f);
            settings.client_cert = Some(file.into());
        }
        "key" => settings.client_key = Some(v.into()),
        "compressed" | "silent" | "show-error" | "verbose" | "include" | "fail"
        | "fail-with-body" | "globoff" | "no-buffer" | "progress-bar" | "no-progress-meter"
        | "path-as-is" | "ipv4" | "ipv6" | "tlsv1" | "tlsv1.2" | "tlsv1.3" | "ssl"
        | "remote-name" | "remote-header-name" | "no-keepalive" | "tr-encoding" | "noproxy" => {}
        "output"
        | "write-out"
        | "cookie-jar"
        | "dump-header"
        | "retry"
        | "retry-delay"
        | "retry-max-time"
        | "interface"
        | "limit-rate"
        | "trace"
        | "trace-ascii"
        | "stderr"
        | "speed-time"
        | "speed-limit"
        | "cert-type"
        | "key-type"
        | "pass"
        | "capath"
        | "ciphers"
        | "keepalive-time"
        | "expect100-timeout"
        | "happy-eyeballs-timeout-ms" => {}
        "proxy-user" => imported.warn("curl: proxy credentials (`--proxy-user`) are not imported"),
        other => imported.warn(format!("curl: ignored option `--{other}`")),
    }
}

fn seconds_ms(v: &str) -> Option<u64> {
    let secs: f64 = v.parse().ok()?;
    (secs.is_finite() && secs >= 0.0).then(|| (secs * 1000.0).round() as u64)
}

/// `-F name=value`, `name=@file;type=…;filename=…` or `name=<file`.
fn form_part(imported: &mut Imported, spec: &str, special: bool) -> PartSpec {
    let (name, rest) = spec.split_once('=').unwrap_or((spec, ""));
    let mut part = PartSpec {
        name: name.to_owned(),
        value: None,
        file: None,
        filename: None,
        content_type: None,
        enabled: true,
    };
    if special && (rest.starts_with('@') || rest.starts_with('<')) {
        let mut pieces = rest[1..].split(';');
        let path = pieces.next().unwrap_or_default();
        for attr in pieces {
            match attr.split_once('=') {
                Some(("type", t)) => part.content_type = Some(t.to_owned()),
                Some(("filename", f)) => part.filename = Some(f.trim_matches('"').to_owned()),
                _ => {}
            }
        }
        if rest.starts_with('@') {
            part.file = Some(path.into());
            imported.warn(format!(
                "curl: the file part `{name}` refers to `{path}`, relative to the request file"
            ));
        } else {
            part.value = Some(String::new());
            imported.warn(format!(
                "curl: the part `{name}` reads its value from `{path}`; it was left empty"
            ));
        }
    } else {
        let mut pieces = rest.split(';');
        part.value = Some(pieces.next().unwrap_or_default().to_owned());
        if special {
            for attr in pieces {
                if let Some(("type", t)) = attr.split_once('=') {
                    part.content_type = Some(t.to_owned());
                }
            }
        }
    }
    part
}

fn finish(imported: &mut Imported, mut p: Parsed) -> Result<RequestFile> {
    let Some(url) = p.urls.first().cloned() else {
        return Err(Error::invalid(FORMAT, "a curl command without a URL"));
    };
    if p.urls.len() > 1 {
        imported.warn("curl: only the first URL of a command is imported");
    }
    let mut headers: KeyValues = std::mem::take(&mut p.headers).into();
    let mut url = url;
    let mut body = None;
    if !p.data.is_empty() {
        let data = p.data.join("&");
        if p.get {
            let sep = if url.contains('?') { '&' } else { '?' };
            url = format!("{url}{sep}{data}");
        } else if p.data_is_json {
            body = Some(BodySpec::Json {
                content: Some(data),
                file: None,
            });
        } else {
            let ct = header(&headers, "content-type")
                .unwrap_or("application/x-www-form-urlencoded")
                .to_owned();
            body = Some(text_body(Some(&ct), &data));
        }
    } else if !p.form.is_empty() {
        body = Some(BodySpec::Multipart {
            parts: std::mem::take(&mut p.form),
        });
    }
    let method = p.method.clone().unwrap_or_else(|| {
        if p.head {
            "HEAD".into()
        } else if body.is_some() {
            "POST".into()
        } else {
            "GET".into()
        }
    });
    if http::Method::from_bytes(method.as_bytes()).is_err() {
        return Err(Error::invalid(FORMAT, format!("invalid method `{method}`")));
    }
    if !url.contains("://") && !url.starts_with("{{") {
        url = format!("http://{url}");
    }

    let mut auth = None;
    if let Some(user) = &p.user {
        let (name, password) = user.split_once(':').unwrap_or((user, ""));
        let password = imported.secret("password", password);
        auth = Some(if let Some(spec) = &p.aws_sigv4 {
            // `aws:amz:REGION:SERVICE`
            let parts: Vec<&str> = spec.split(':').collect();
            AuthSpec::AwsSigV4 {
                access_key: name.to_owned(),
                secret_key: password,
                session_token: String::new(),
                region: parts.get(2).copied().unwrap_or("us-east-1").to_owned(),
                service: parts.get(3).copied().unwrap_or("execute-api").to_owned(),
            }
        } else if p.digest {
            AuthSpec::Digest {
                username: name.to_owned(),
                password,
            }
        } else {
            AuthSpec::Basic {
                username: name.to_owned(),
                password,
            }
        });
    } else if let Some(token) = &p.bearer {
        auth = Some(AuthSpec::Bearer {
            token: imported.secret("token", token),
        });
    }
    protect_headers(imported, &mut headers, &mut auth);
    tidy_headers(&mut headers, body.as_ref());

    let mut r = std::mem::replace(&mut p.request, RequestFile::new("GET", ""));
    r.name = Some(title(&method, &url));
    r.method = (method != "GET").then_some(method);
    r.url = url;
    r.headers = headers;
    r.auth = auth;
    r.body = body;
    Ok(r)
}

/// `GET users` from the method and the URL's last path segment.
pub(crate) fn title(method: &str, url: &str) -> String {
    let path = url.split(['?', '#']).next().unwrap_or(url);
    let path = path.split_once("://").map_or(path, |(_, rest)| rest);
    let tail = path
        .trim_end_matches('/')
        .split_once('/')
        .map_or("", |(_, p)| p);
    if tail.is_empty() {
        format!("{method} {}", path.trim_end_matches('/'))
    } else {
        format!("{method} /{tail}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(text: &str) -> Vec<String> {
        tokenize(text)
            .unwrap()
            .into_iter()
            .filter_map(|t| match t {
                Token::Word(w) => Some(w),
                Token::Separator => None,
            })
            .collect()
    }

    #[test]
    fn shell_quoting() {
        assert_eq!(
            words(r#"curl 'a b' "c \"d\" \$e" $'f\ng\x41' h\ i"#),
            ["curl", "a b", "c \"d\" $e", "f\ngA", "h i"]
        );
        assert_eq!(words("curl \\\n  -s \\\r\n x"), ["curl", "-s", "x"]);
        assert_eq!(words("curl ^\r\n -s"), ["curl", "-s"]);
        assert!(tokenize("curl 'open").is_err());
    }

    #[test]
    fn several_commands() {
        let cmds =
            split_commands(&tokenize("$ curl a.test # first\ncurl b.test && curl c.test").unwrap());
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0], ["curl", "a.test"]);
    }

    #[test]
    fn titles() {
        assert_eq!(title("GET", "https://a.test/v1/users?x=1"), "GET /v1/users");
        assert_eq!(title("GET", "https://a.test/"), "GET a.test");
    }
}
