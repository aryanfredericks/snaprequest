//! Requests as code (PLAN P9.2): a cURL command or a Python (requests), JavaScript (fetch),
//! Go (net/http) or Rust (reqwest) snippet.
//!
//! Secret values are never written: wherever one would appear, the snippet reads an environment
//! variable instead (`$SNAPREQ_SECRET_TOKEN` for the secret variable `token`, the name the CLI
//! also reads secrets from). Basic auth is therefore written as credentials, not as a
//! pre-encoded header.

use std::fmt::Write as _;
use std::str::FromStr;

use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use snapreq_core::Request;
use snapreq_core::model::{ApiKeyLocation, Auth, Body, PartValue};

/// A language or tool a request is exported to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Curl,
    Python,
    JavaScript,
    Go,
    Rust,
}

impl Target {
    pub const ALL: [Self; 5] = [
        Self::Curl,
        Self::Python,
        Self::JavaScript,
        Self::Go,
        Self::Rust,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Curl => "curl",
            Self::Python => "python",
            Self::JavaScript => "javascript",
            Self::Go => "go",
            Self::Rust => "rust",
        }
    }

    fn comment(self) -> &'static str {
        match self {
            Self::Curl | Self::Python => "#",
            _ => "//",
        }
    }
}

impl std::fmt::Display for Target {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Target {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "curl" => Ok(Self::Curl),
            "python" | "py" => Ok(Self::Python),
            "javascript" | "js" | "fetch" => Ok(Self::JavaScript),
            "go" | "golang" => Ok(Self::Go),
            "rust" | "reqwest" => Ok(Self::Rust),
            other => Err(format!(
                "unknown export format `{other}` (expected one of: {})",
                Self::ALL.map(Self::as_str).join(", ")
            )),
        }
    }
}

/// A piece of a string: literal text, or the value of an environment variable.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Part {
    Lit(String),
    Env(String),
}

/// A string that may read environment variables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Text(Vec<Part>);

impl Text {
    fn lit(s: &str) -> Self {
        Self(vec![Part::Lit(s.to_owned())])
    }

    fn has_env(&self) -> bool {
        self.0.iter().any(|p| matches!(p, Part::Env(_)))
    }

    fn push(&mut self, other: Self) {
        for p in other.0 {
            match (self.0.last_mut(), p) {
                (Some(Part::Lit(a)), Part::Lit(b)) => a.push_str(&b),
                (_, p) => self.0.push(p),
            }
        }
    }

    fn join(parts: impl IntoIterator<Item = Self>) -> Self {
        let mut out = Self::default();
        for p in parts {
            out.push(p);
        }
        out
    }

    fn map_lit(self, f: impl Fn(&str) -> String) -> Self {
        Self(
            self.0
                .into_iter()
                .map(|p| match p {
                    Part::Lit(s) => Part::Lit(f(&s)),
                    env => env,
                })
                .collect(),
        )
    }
}

/// Secret values and the environment variables that replace them.
struct Secrets(Vec<(String, String)>);

impl Secrets {
    fn new(req: &Request, named: &[(String, String)]) -> Self {
        let mut out: Vec<(String, String)> = Vec::new();
        for (name, value) in named {
            if !value.is_empty() && !out.iter().any(|(v, _)| v == value) {
                out.push((
                    value.clone(),
                    snapreq_collection::secrets::env_var_name(name),
                ));
            }
        }
        let mut n = 0;
        for value in req.secrets() {
            if !value.is_empty() && !out.iter().any(|(v, _)| *v == value) {
                n += 1;
                out.push((value, format!("SNAPREQ_SECRET_{n}")));
            }
        }
        // Longest first, so a secret containing another is replaced whole.
        out.sort_by_key(|(v, _)| std::cmp::Reverse(v.len()));
        Self(out)
    }

    fn text(&self, s: &str) -> Text {
        let mut parts = vec![Part::Lit(s.to_owned())];
        for (value, env) in &self.0 {
            let mut next = Vec::new();
            for p in parts {
                match p {
                    Part::Lit(lit) if lit.contains(value.as_str()) => {
                        let mut pieces = lit.split(value.as_str()).peekable();
                        while let Some(piece) = pieces.next() {
                            if !piece.is_empty() {
                                next.push(Part::Lit(piece.to_owned()));
                            }
                            if pieces.peek().is_some() {
                                next.push(Part::Env(env.clone()));
                            }
                        }
                    }
                    other => next.push(other),
                }
            }
            parts = next;
        }
        Text(parts)
    }
}

/// Characters encoded in query names and values.
const QUERY: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'&')
    .add(b'+')
    .add(b'<')
    .add(b'=')
    .add(b'>')
    .add(b'%');

enum BodyOut {
    Raw(Text),
    Form(Vec<(Text, Text)>),
    Multipart(Vec<PartOut>),
    File(String),
}

enum PartOut {
    Text(String, Text),
    File {
        name: String,
        path: String,
        filename: Option<String>,
        content_type: Option<String>,
    },
}

/// A request reduced to what every target can express.
struct Spec {
    method: String,
    url: Text,
    headers: Vec<(String, Text)>,
    basic: Option<(Text, Text)>,
    digest: Option<(Text, Text)>,
    aws: Option<(Text, Text, String, String)>,
    body: Option<BodyOut>,
    notes: Vec<String>,
    timeout_ms: Option<u128>,
    insecure: bool,
    follow: bool,
}

fn spec(req: &Request, secrets: &Secrets) -> Spec {
    let t = |s: &str| secrets.text(s);
    let mut url = t(req.url.trim());
    let mut query: Vec<(Text, Text)> = req
        .query
        .iter()
        .filter(|q| q.enabled)
        .map(|q| (t(&q.name), t(&q.value)))
        .collect();
    let mut headers: Vec<(String, Text)> = req
        .enabled_headers()
        .map(|h| (h.name.clone(), t(&h.value)))
        .collect();
    let has = |headers: &[(String, Text)], name: &str| {
        headers.iter().any(|(n, _)| n.eq_ignore_ascii_case(name))
    };
    let mut notes = Vec::new();
    let (mut basic, mut digest, mut aws) = (None, None, None);
    match &req.auth {
        Auth::None => {}
        Auth::Basic { username, password } => basic = Some((t(username), t(password))),
        Auth::Digest { username, password } => digest = Some((t(username), t(password))),
        Auth::Bearer { token } => {
            if !has(&headers, "authorization") {
                headers.push((
                    "Authorization".into(),
                    Text::join([Text::lit("Bearer "), t(token)]),
                ));
            }
        }
        Auth::ApiKey {
            name,
            value,
            location,
        } => match location {
            ApiKeyLocation::Header => {
                if !has(&headers, name) {
                    headers.push((name.clone(), t(value)));
                }
            }
            ApiKeyLocation::Query => query.push((t(name), t(value))),
        },
        Auth::OAuth2(o) => {
            notes.push(format!(
                "OAuth 2.0 ({}): get an access token from {} and put it in SNAPREQ_ACCESS_TOKEN.",
                o.grant.as_str(),
                o.token_url
            ));
            if !has(&headers, "authorization") {
                headers.push((
                    "Authorization".into(),
                    Text(vec![
                        Part::Lit("Bearer ".into()),
                        Part::Env("SNAPREQ_ACCESS_TOKEN".into()),
                    ]),
                ));
            }
        }
        Auth::AwsSigV4 {
            access_key,
            secret_key,
            session_token,
            region,
            service,
        } => {
            if !session_token.is_empty() && !has(&headers, "x-amz-security-token") {
                headers.push(("X-Amz-Security-Token".into(), t(session_token)));
            }
            aws = Some((
                t(access_key),
                t(secret_key),
                region.clone(),
                service.clone(),
            ));
        }
    }
    if !query.is_empty() {
        let mut sep = if req.url.contains('?') { "&" } else { "?" };
        for (k, v) in query {
            let enc = |s: &str| utf8_percent_encode(s, QUERY).to_string();
            url.push(Text::join([
                Text::lit(sep),
                k.map_lit(enc),
                Text::lit("="),
                v.map_lit(enc),
            ]));
            sep = "&";
        }
    }
    let body = match &req.body {
        Body::None => None,
        Body::Text { content, .. } | Body::Json { content } => Some(BodyOut::Raw(t(content))),
        Body::Form { fields } => Some(BodyOut::Form(
            fields
                .iter()
                .filter(|f| f.enabled)
                .map(|f| (t(&f.name), t(&f.value)))
                .collect(),
        )),
        Body::Multipart { parts } => Some(BodyOut::Multipart(
            parts
                .iter()
                .filter(|p| p.enabled)
                .map(|p| match &p.value {
                    PartValue::Text { value } => PartOut::Text(p.name.clone(), t(value)),
                    PartValue::File {
                        path,
                        filename,
                        content_type,
                    } => PartOut::File {
                        name: p.name.clone(),
                        path: path.to_string_lossy().into_owned(),
                        filename: filename.clone(),
                        content_type: content_type.clone(),
                    },
                })
                .collect(),
        )),
        Body::File { path, .. } => Some(BodyOut::File(path.to_string_lossy().into_owned())),
    };
    if let Some(ct) = req.body.default_content_type()
        && !has(&headers, "content-type")
    {
        headers.push(("Content-Type".into(), Text::lit(ct)));
    }
    Spec {
        method: req.method.as_str().to_owned(),
        url,
        headers,
        basic,
        digest,
        aws,
        body,
        notes,
        // Only a timeout other than SnapRequest's default is worth writing down.
        timeout_ms: req
            .settings
            .timeout
            .filter(|t| Some(*t) != snapreq_core::model::RequestSettings::default().timeout)
            .map(|d| d.as_millis()),
        insecure: !req.settings.tls.verify,
        follow: req.settings.follow_redirects,
    }
}

/// The request as code for `target`. `named` maps secret variable names to their values, so
/// the snippet can read `SNAPREQ_SECRET_<NAME>`; other secrets of the request (credentials
/// written into it) are read from numbered variables.
pub fn snippet(req: &Request, target: Target, named: &[(String, String)]) -> String {
    let secrets = Secrets::new(req, named);
    let s = spec(req, &secrets);
    let mut out = String::new();
    for note in &s.notes {
        let _ = writeln!(out, "{} {note}", target.comment());
    }
    let envs: Vec<String> = secrets
        .0
        .iter()
        .map(|(_, e)| e.clone())
        .chain(s.notes.iter().map(|_| "SNAPREQ_ACCESS_TOKEN".to_owned()))
        .collect();
    let code = match target {
        Target::Curl => curl(&s),
        Target::Python => python(&s, target),
        Target::JavaScript => javascript(&s, target),
        Target::Go => go(&s, target),
        Target::Rust => rust(&s, target),
    };
    let used: Vec<&String> = {
        let mut u: Vec<&String> = envs.iter().filter(|e| code.contains(e.as_str())).collect();
        u.dedup();
        u
    };
    if !used.is_empty() && target != Target::Curl {
        let mut names: Vec<&str> = used.iter().map(|s| s.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        let _ = writeln!(
            out,
            "{} Secret values are read from the environment: {}.",
            target.comment(),
            names.join(", ")
        );
    }
    out.push_str(&code);
    // Defence in depth: no secret value may survive in any form.
    for (value, _) in &secrets.0 {
        if value.len() >= 3 && out.contains(value.as_str()) {
            out = out.replace(value.as_str(), "<redacted>");
        }
    }
    out
}

// --- cURL ------------------------------------------------------------------------------------

/// A shell word: single-quoted, or double-quoted with `${VAR}` when it reads the environment.
fn sh(t: &Text) -> String {
    if !t.has_env() {
        let s: String =
            t.0.iter()
                .map(|p| match p {
                    Part::Lit(s) => s.as_str(),
                    Part::Env(_) => "",
                })
                .collect();
        return format!("'{}'", s.replace('\'', r"'\''"));
    }
    let mut out = String::from("\"");
    for p in &t.0 {
        match p {
            Part::Lit(s) => {
                for c in s.chars() {
                    if matches!(c, '"' | '\\' | '$' | '`') {
                        out.push('\\');
                    }
                    out.push(c);
                }
            }
            Part::Env(e) => {
                let _ = write!(out, "${{{e}}}");
            }
        }
    }
    out.push('"');
    out
}

fn curl(s: &Spec) -> String {
    let mut args: Vec<String> = Vec::new();
    let default_method = match &s.body {
        Some(_) => "POST",
        None => "GET",
    };
    if s.method == "HEAD" {
        args.push("--head".into());
    } else if s.method != default_method {
        args.push(format!("-X {}", s.method));
    }
    args.push(sh(&s.url));
    for (name, value) in &s.headers {
        args.push(format!(
            "-H {}",
            sh(&Text::join([
                Text::lit(&format!("{name}: ")),
                value.clone()
            ]))
        ));
    }
    if let Some((u, p)) = s.basic.as_ref().or(s.digest.as_ref()) {
        args.push(format!(
            "-u {}",
            sh(&Text::join([u.clone(), Text::lit(":"), p.clone()]))
        ));
        if s.digest.is_some() {
            args.push("--digest".into());
        }
    }
    if let Some((key, secret, region, service)) = &s.aws {
        args.push(format!("--aws-sigv4 'aws:amz:{region}:{service}'"));
        args.push(format!(
            "-u {}",
            sh(&Text::join([key.clone(), Text::lit(":"), secret.clone()]))
        ));
    }
    match &s.body {
        None => {}
        Some(BodyOut::Raw(t)) => args.push(format!("--data-raw {}", sh(t))),
        Some(BodyOut::Form(fields)) => {
            for (k, v) in fields {
                args.push(format!(
                    "--data-urlencode {}",
                    sh(&Text::join([k.clone(), Text::lit("="), v.clone()]))
                ));
            }
        }
        Some(BodyOut::Multipart(parts)) => {
            for p in parts {
                match p {
                    PartOut::Text(name, v) => args.push(format!(
                        "--form-string {}",
                        sh(&Text::join([Text::lit(&format!("{name}=")), v.clone()]))
                    )),
                    PartOut::File {
                        name,
                        path,
                        filename,
                        content_type,
                    } => {
                        let mut spec = format!("{name}=@{path}");
                        if let Some(t) = content_type {
                            let _ = write!(spec, ";type={t}");
                        }
                        if let Some(f) = filename {
                            let _ = write!(spec, ";filename={f}");
                        }
                        args.push(format!("-F {}", sh(&Text::lit(&spec))));
                    }
                }
            }
        }
        Some(BodyOut::File(path)) => {
            args.push(format!(
                "--data-binary {}",
                sh(&Text::lit(&format!("@{path}")))
            ));
        }
    }
    if s.follow {
        args.push("-L".into());
    }
    if s.insecure {
        args.push("-k".into());
    }
    if let Some(ms) = s.timeout_ms {
        args.push(format!("-m {}", ms as f64 / 1000.0));
    }
    let mut out = String::from("curl");
    for a in args {
        out.push_str(" \\\n  ");
        out.push_str(&a);
    }
    out.push('\n');
    out
}

// --- Python, JavaScript and Go: double-quoted strings joined with `+` ----------------------

/// A JSON-style string literal, valid in Python, JavaScript and Go.
fn quoted(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c == '\u{2028}' || c == '\u{2029}' => {
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn concat(t: &Text, env: impl Fn(&str) -> String) -> String {
    if t.0.is_empty() {
        return "\"\"".into();
    }
    t.0.iter()
        .map(|p| match p {
            Part::Lit(s) => quoted(s),
            Part::Env(e) => env(e),
        })
        .collect::<Vec<_>>()
        .join(" + ")
}

fn py(t: &Text) -> String {
    concat(t, |e| format!("os.environ[{}]", quoted(e)))
}

fn python(s: &Spec, target: Target) -> String {
    let mut imports = vec!["import requests"];
    let mut args: Vec<String> = vec![quoted(&s.method), py(&s.url)];
    if !s.headers.is_empty() {
        let mut h = String::from("headers={\n");
        for (name, value) in merged(&s.headers) {
            let _ = writeln!(h, "        {}: {},", quoted(&name), py(&value));
        }
        h.push_str("    }");
        args.push(h);
    }
    let mut setup = String::new();
    match &s.body {
        None => {}
        Some(BodyOut::Raw(t)) => args.push(format!("data={}", py(t))),
        Some(BodyOut::Form(fields)) => args.push(format!(
            "data=[{}]",
            fields
                .iter()
                .map(|(k, v)| format!("({}, {})", py(k), py(v)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Some(BodyOut::Multipart(parts)) => {
            let mut data = Vec::new();
            let mut files = Vec::new();
            for p in parts {
                match p {
                    PartOut::Text(name, v) => data.push(format!("({}, {})", quoted(name), py(v))),
                    PartOut::File {
                        name,
                        path,
                        filename,
                        content_type,
                    } => {
                        let fname = filename
                            .clone()
                            .unwrap_or_else(|| file_name(path).to_owned());
                        files.push(format!(
                            "({}, ({}, open({}, \"rb\"){}))",
                            quoted(name),
                            quoted(&fname),
                            quoted(path),
                            content_type
                                .as_deref()
                                .map(|c| format!(", {}", quoted(c)))
                                .unwrap_or_default()
                        ));
                    }
                }
            }
            if !data.is_empty() {
                args.push(format!("data=[{}]", data.join(", ")));
            }
            args.push(format!("files=[{}]", files.join(", ")));
        }
        Some(BodyOut::File(path)) => {
            let _ = writeln!(setup, "body = open({}, \"rb\")\n", quoted(path));
            args.push("data=body".into());
        }
    }
    if let Some((u, p)) = &s.basic {
        args.push(format!("auth=({}, {})", py(u), py(p)));
    }
    if let Some((u, p)) = &s.digest {
        imports.push("from requests.auth import HTTPDigestAuth");
        args.push(format!("auth=HTTPDigestAuth({}, {})", py(u), py(p)));
    }
    if s.aws.is_some() {
        let _ = writeln!(
            setup,
            "{} AWS Signature V4: sign the request, for example with the requests-aws4auth package.\n",
            target.comment()
        );
    }
    if let Some(ms) = s.timeout_ms {
        args.push(format!("timeout={}", ms as f64 / 1000.0));
    }
    if s.insecure {
        args.push("verify=False".into());
    }
    if !s.follow {
        args.push("allow_redirects=False".into());
    }
    let body_text = format!(
        "{setup}response = requests.request(\n{})\n\nprint(response.status_code)\nprint(response.text)\n",
        args.iter()
            .map(|a| format!("    {a},\n"))
            .collect::<String>()
    );
    if body_text.contains("os.environ[") {
        imports.insert(0, "import os");
    }
    format!("{}\n\n{body_text}", imports.join("\n"))
}

/// Header values of the same name joined with `, ` (dictionaries hold one value per name).
fn merged(headers: &[(String, Text)]) -> Vec<(String, Text)> {
    let mut out: Vec<(String, Text)> = Vec::new();
    for (name, value) in headers {
        match out.iter_mut().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some((_, v)) => {
                v.push(Text::lit(", "));
                v.push(value.clone());
            }
            None => out.push((name.clone(), value.clone())),
        }
    }
    out
}

fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn js(t: &Text) -> String {
    concat(t, |e| format!("process.env.{e}"))
}

fn javascript(s: &Spec, target: Target) -> String {
    let mut setup = String::new();
    let mut opts: Vec<String> = vec![format!("method: {}", quoted(&s.method))];
    let mut headers: Vec<(String, String)> = merged(&s.headers)
        .iter()
        .map(|(n, v)| (n.clone(), js(v)))
        .collect();
    if let Some((u, p)) = &s.basic {
        headers.push((
            "Authorization".into(),
            format!(
                "\"Basic \" + btoa({})",
                js(&Text::join([u.clone(), Text::lit(":"), p.clone()]))
            ),
        ));
    }
    if !headers.is_empty() {
        let mut h = String::from("headers: {\n");
        for (name, value) in &headers {
            let _ = writeln!(h, "    {}: {value},", quoted(name));
        }
        h.push_str("  }");
        opts.push(h);
    }
    let mut uses_fs = false;
    match &s.body {
        None => {}
        Some(BodyOut::Raw(t)) => opts.push(format!("body: {}", js(t))),
        Some(BodyOut::Form(fields)) => opts.push(format!(
            "body: new URLSearchParams([{}])",
            fields
                .iter()
                .map(|(k, v)| format!("[{}, {}]", js(k), js(v)))
                .collect::<Vec<_>>()
                .join(", ")
        )),
        Some(BodyOut::Multipart(parts)) => {
            setup.push_str("const form = new FormData();\n");
            for p in parts {
                match p {
                    PartOut::Text(name, v) => {
                        let _ = writeln!(setup, "form.append({}, {});", quoted(name), js(v));
                    }
                    PartOut::File {
                        name,
                        path,
                        filename,
                        content_type,
                    } => {
                        uses_fs = true;
                        let fname = filename
                            .clone()
                            .unwrap_or_else(|| file_name(path).to_owned());
                        let opts = content_type
                            .as_deref()
                            .map(|c| format!(", {{ type: {} }}", quoted(c)))
                            .unwrap_or_default();
                        let _ = writeln!(
                            setup,
                            "form.append({}, await fs.openAsBlob({}{opts}), {});",
                            quoted(name),
                            quoted(path),
                            quoted(&fname)
                        );
                    }
                }
            }
            setup.push('\n');
            opts.push("body: form".into());
        }
        Some(BodyOut::File(path)) => {
            uses_fs = true;
            opts.push(format!(
                "body: await fs.promises.readFile({})",
                quoted(path)
            ));
        }
    }
    if !s.follow {
        opts.push("redirect: \"manual\"".into());
    }
    if let Some(ms) = s.timeout_ms {
        opts.push(format!("signal: AbortSignal.timeout({ms})"));
    }
    let mut notes = String::new();
    if s.digest.is_some() {
        let _ = writeln!(
            notes,
            "{} Digest auth is not built into fetch; the request is sent without it.",
            target.comment()
        );
    }
    if s.aws.is_some() {
        let _ = writeln!(
            notes,
            "{} AWS Signature V4: sign the request, for example with @aws-sdk/signature-v4.",
            target.comment()
        );
    }
    if s.insecure {
        let _ = writeln!(
            notes,
            "{} TLS certificates are not verified by the original request: run with NODE_TLS_REJECT_UNAUTHORIZED=0.",
            target.comment()
        );
    }
    let import = if uses_fs {
        "import fs from \"node:fs\";\n\n"
    } else {
        ""
    };
    format!(
        "{import}{notes}{setup}const response = await fetch({}, {{\n{}}});\n\nconsole.log(response.status);\nconsole.log(await response.text());\n",
        js(&s.url),
        opts.iter().map(|o| format!("  {o},\n")).collect::<String>()
    )
}

fn go_str(t: &Text) -> String {
    concat(t, |e| format!("os.Getenv({})", quoted(e)))
}

fn go(s: &Spec, target: Target) -> String {
    let mut imports = vec!["fmt", "io", "net/http"];
    let mut setup = String::new();
    let body_var = match &s.body {
        None => "nil".to_owned(),
        Some(BodyOut::Raw(t)) => {
            imports.push("strings");
            format!("strings.NewReader({})", go_str(t))
        }
        Some(BodyOut::Form(fields)) => {
            imports.push("net/url");
            imports.push("strings");
            setup.push_str("\tform := url.Values{}\n");
            for (k, v) in fields {
                let _ = writeln!(setup, "\tform.Add({}, {})", go_str(k), go_str(v));
            }
            "strings.NewReader(form.Encode())".into()
        }
        Some(BodyOut::Multipart(parts)) => {
            imports.push("bytes");
            imports.push("mime/multipart");
            setup.push_str("\tvar buf bytes.Buffer\n\tw := multipart.NewWriter(&buf)\n");
            for p in parts {
                match p {
                    PartOut::Text(name, v) => {
                        let _ = writeln!(setup, "\tw.WriteField({}, {})", quoted(name), go_str(v));
                    }
                    PartOut::File {
                        name,
                        path,
                        filename,
                        ..
                    } => {
                        imports.push("os");
                        let fname = filename
                            .clone()
                            .unwrap_or_else(|| file_name(path).to_owned());
                        let _ = write!(
                            setup,
                            "\t{{\n\t\tf, err := os.Open({})\n\t\tif err != nil {{\n\t\t\tpanic(err)\n\t\t}}\n\t\tpart, _ := w.CreateFormFile({}, {})\n\t\tio.Copy(part, f)\n\t\tf.Close()\n\t}}\n",
                            quoted(path),
                            quoted(name),
                            quoted(&fname)
                        );
                    }
                }
            }
            setup.push_str("\tw.Close()\n");
            "&buf".into()
        }
        Some(BodyOut::File(path)) => {
            imports.push("os");
            let _ = write!(
                setup,
                "\tbody, err := os.Open({})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tdefer body.Close()\n",
                quoted(path)
            );
            "body".into()
        }
    };
    let mut code = setup;
    let _ = write!(
        code,
        "\treq, err := http.NewRequest({}, {}, {body_var})\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n",
        quoted(&s.method),
        go_str(&s.url)
    );
    for (name, value) in &s.headers {
        if matches!(s.body, Some(BodyOut::Multipart(_)))
            && name.eq_ignore_ascii_case("content-type")
        {
            continue;
        }
        let _ = writeln!(
            code,
            "\treq.Header.Add({}, {})",
            quoted(name),
            go_str(value)
        );
    }
    if matches!(s.body, Some(BodyOut::Multipart(_))) {
        code.push_str("\treq.Header.Set(\"Content-Type\", w.FormDataContentType())\n");
    }
    if let Some((u, p)) = &s.basic {
        let _ = writeln!(code, "\treq.SetBasicAuth({}, {})", go_str(u), go_str(p));
    }
    if s.digest.is_some() {
        let _ = writeln!(
            code,
            "\t{} Digest auth is not built into net/http; the request is sent without it.",
            target.comment()
        );
    }
    if s.aws.is_some() {
        let _ = writeln!(
            code,
            "\t{} AWS Signature V4: sign the request with aws-sdk-go-v2's v4.Signer.",
            target.comment()
        );
    }
    let mut client = Vec::new();
    if let Some(ms) = s.timeout_ms {
        imports.push("time");
        client.push(format!("Timeout: {ms} * time.Millisecond"));
    }
    if s.insecure {
        imports.push("crypto/tls");
        client.push(
            "Transport: &http.Transport{TLSClientConfig: &tls.Config{InsecureSkipVerify: true}}"
                .into(),
        );
    }
    if !s.follow {
        client.push(
            "CheckRedirect: func(*http.Request, []*http.Request) error { return http.ErrUseLastResponse }"
                .into(),
        );
    }
    let _ = write!(
        code,
        "\tclient := &http.Client{{{}}}\n\tresp, err := client.Do(req)\n\tif err != nil {{\n\t\tpanic(err)\n\t}}\n\tdefer resp.Body.Close()\n\tdata, _ := io.ReadAll(resp.Body)\n\tfmt.Println(resp.Status)\n\tfmt.Println(string(data))\n",
        client
            .iter()
            .map(|c| format!("\n\t\t{c},"))
            .collect::<String>()
            + if client.is_empty() { "" } else { "\n\t" }
    );
    if code.contains("os.Getenv(") {
        imports.push("os");
    }
    imports.sort_unstable();
    imports.dedup();
    format!(
        "package main\n\nimport (\n{})\n\nfunc main() {{\n{code}}}\n",
        imports
            .iter()
            .map(|i| format!("\t{}\n", quoted(i)))
            .collect::<String>()
    )
}

/// A Rust expression for the text: a literal, or `format!` reading the environment.
fn rs(t: &Text) -> String {
    if !t.has_env() {
        let s: String =
            t.0.iter()
                .map(|p| match p {
                    Part::Lit(s) => s.as_str(),
                    Part::Env(_) => "",
                })
                .collect();
        return format!("{s:?}");
    }
    let mut fmt = String::new();
    let mut args = Vec::new();
    for p in &t.0 {
        match p {
            Part::Lit(s) => fmt.push_str(&s.replace('{', "{{").replace('}', "}}")),
            Part::Env(e) => {
                fmt.push_str("{}");
                args.push(format!("std::env::var({e:?})?"));
            }
        }
    }
    if fmt == "{}" {
        return args.remove(0);
    }
    format!("format!({fmt:?}, {})", args.join(", "))
}

/// [`rs`] as a `String` in every case, for arrays mixing literals and environment values.
fn rs_owned(t: &Text) -> String {
    if t.has_env() {
        rs(t)
    } else {
        format!("{}.to_owned()", rs(t))
    }
}

fn rust(s: &Spec, target: Target) -> String {
    let mut features = Vec::new();
    let mut builder = vec!["reqwest::Client::builder()".to_owned()];
    if let Some(ms) = s.timeout_ms {
        builder.push(format!(".timeout(std::time::Duration::from_millis({ms}))"));
    }
    if s.insecure {
        builder.push(".danger_accept_invalid_certs(true)".into());
    }
    if !s.follow {
        builder.push(".redirect(reqwest::redirect::Policy::none())".into());
    }
    builder.push(".build()?".into());
    let mut setup = String::new();
    let mut chain = vec![format!(
        ".request(reqwest::Method::from_bytes({:?}.as_bytes())?, {})",
        s.method,
        rs(&s.url)
    )];
    for (name, value) in &s.headers {
        if matches!(s.body, Some(BodyOut::Multipart(_)))
            && name.eq_ignore_ascii_case("content-type")
        {
            continue;
        }
        chain.push(format!(".header({name:?}, {})", rs(value)));
    }
    if let Some((u, p)) = &s.basic {
        chain.push(format!(".basic_auth({}, Some({}))", rs(u), rs(p)));
    }
    match &s.body {
        None => {}
        Some(BodyOut::Raw(t)) => chain.push(format!(".body({})", rs(t))),
        Some(BodyOut::Form(fields)) => {
            features.push("form");
            chain.push(format!(
                ".form(&[{}])",
                fields
                    .iter()
                    .map(|(k, v)| format!("({}, {})", rs_owned(k), rs_owned(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Some(BodyOut::Multipart(parts)) => {
            features.push("multipart");
            features.push("stream");
            let mut form = String::from("    let form = reqwest::multipart::Form::new()");
            for p in parts {
                match p {
                    PartOut::Text(name, v) => {
                        let _ = write!(form, "\n        .text({name:?}, {})", rs(v));
                    }
                    PartOut::File { name, path, .. } => {
                        let _ =
                            write!(form, "\n        .file({name:?}, {path:?})\n        .await?");
                    }
                }
            }
            form.push_str(";\n");
            setup.push_str(&form);
            chain.push(".multipart(form)".into());
        }
        Some(BodyOut::File(path)) => {
            chain.push(format!(".body(tokio::fs::read({path:?}).await?)"));
        }
    }
    let mut notes = String::new();
    if s.digest.is_some() {
        let _ = writeln!(
            notes,
            "    {} Digest auth is not built into reqwest; the request is sent without it.",
            target.comment()
        );
    }
    if s.aws.is_some() {
        let _ = writeln!(
            notes,
            "    {} AWS Signature V4: sign the request with the aws-sigv4 crate.",
            target.comment()
        );
    }
    chain.push(".send()".into());
    chain.push(".await?".into());
    let features = if features.is_empty() {
        String::new()
    } else {
        format!(
            ", features = [{}]",
            features
                .iter()
                .map(|f| format!("{f:?}"))
                .collect::<Vec<_>>()
                .join(", ")
        )
    };
    format!(
        "// Cargo.toml: reqwest = {{ version = \"0.12\"{features} }}, tokio = {{ version = \"1\", features = [\"full\"] }}\n\n#[tokio::main]\nasync fn main() -> Result<(), Box<dyn std::error::Error>> {{\n{notes}    let client = {};\n{setup}    let response = client\n{};\n    println!(\"{{}}\", response.status());\n    println!(\"{{}}\", response.text().await?);\n    Ok(())\n}}\n",
        builder.join("\n        "),
        chain
            .iter()
            .map(|c| format!("        {c}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use snapreq_core::model::KeyValue;

    fn request() -> Request {
        let mut r = Request::new(http::Method::POST, "https://api.test/users?x=1");
        r.query.push(KeyValue::new("q", "a b&c"));
        r.headers.push(KeyValue::new("X-Team", "it's"));
        r.headers.push(KeyValue::new("X-Off", "no").disabled());
        r.body = Body::Json {
            content: "{\"name\": \"snap\"}".into(),
        };
        r.auth = Auth::Bearer {
            token: "s3cr3t-token".into(),
        };
        r
    }

    #[test]
    fn secrets_become_environment_variables() {
        let named = [("token".to_owned(), "s3cr3t-token".to_owned())];
        for target in Target::ALL {
            let code = snippet(&request(), target, &named);
            assert!(!code.contains("s3cr3t-token"), "{target}: {code}");
            assert!(code.contains("SNAPREQ_SECRET_TOKEN"), "{target}: {code}");
            assert!(
                !code.contains("X-Off"),
                "{target}: disabled headers are left out"
            );
        }
        // Without a name, a numbered variable.
        let code = snippet(&request(), Target::Curl, &[]);
        assert!(code.contains("${SNAPREQ_SECRET_1}"), "{code}");
    }

    #[test]
    fn curl_command() {
        let named = [("token".to_owned(), "s3cr3t-token".to_owned())];
        assert_eq!(
            snippet(&request(), Target::Curl, &named),
            "curl \\\n  'https://api.test/users?x=1&q=a%20b%26c' \\\n  -H 'X-Team: it'\\''s' \\\n  -H \"Authorization: Bearer ${SNAPREQ_SECRET_TOKEN}\" \\\n  -H 'Content-Type: application/json' \\\n  --data-raw '{\"name\": \"snap\"}' \\\n  -L\n"
        );
    }

    #[test]
    fn rust_strings_and_format() {
        let t = Text(vec![Part::Lit("a{b}\"".into()), Part::Env("X".into())]);
        assert_eq!(rs(&t), r#"format!("a{{b}}\"{}", std::env::var("X")?)"#);
        assert_eq!(rs(&Text::lit("é\n")), r#""é\n""#);
    }

    #[test]
    fn targets_parse() {
        assert_eq!("js".parse::<Target>().unwrap(), Target::JavaScript);
        assert!("cobol".parse::<Target>().is_err());
    }
}
