//! `snapreq send`: turning command-line arguments into a request.

use std::path::PathBuf;
use std::time::Duration;

use http::Method;
use snapreq_core::Request;
use snapreq_core::model::{
    ApiKeyLocation, Auth, Body, ClientCertificate, HttpVersionPref, KeyValue, MultipartPart,
    PartValue, ProxyMode, ResolveOverride, TlsVersion,
};

use crate::cli::{ApiKeyInArg, SendArgs, TlsVersionArg};

/// A problem with the arguments, reported before anything is sent.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ArgError(pub String);

impl std::fmt::Display for ArgError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

fn err<T>(msg: impl Into<String>) -> Result<T, ArgError> {
    Err(ArgError(msg.into()))
}

/// Builds the request for `snapreq send [METHOD] URL`.
pub(crate) fn build_request(args: &SendArgs) -> Result<Request, ArgError> {
    let has_body = !body(args)?.is_none();
    let (method, url) = match args.target.as_slice() {
        [url] => (
            if has_body { Method::POST } else { Method::GET },
            url.clone(),
        ),
        [method, url] => {
            let method = Method::from_bytes(method.to_ascii_uppercase().as_bytes())
                .map_err(|_| ArgError(format!("invalid HTTP method `{method}`")))?;
            (method, url.clone())
        }
        _ => return err("expected `[METHOD] URL`"),
    };
    let mut req = Request::new(method, with_default_scheme(&url));
    apply_overrides(&mut req, args)?;
    Ok(req)
}

/// Applies the options given on the command line on top of `req`. Options that were
/// not given leave `req` unchanged, so settings from collection files are kept.
///
/// - `-H` replaces headers of the same name (case-insensitive) and adds new ones;
/// - `-q` adds query parameters;
/// - a body option replaces the body;
/// - an auth option (`-u`, `--bearer`, `--api-key`, `--no-auth`) replaces the auth;
/// - every connection, TLS and output option overrides the matching setting.
pub(crate) fn apply_overrides(req: &mut Request, args: &SendArgs) -> Result<(), ArgError> {
    for h in &args.headers {
        let (name, value) = h
            .split_once(':')
            .ok_or_else(|| ArgError(format!("invalid header `{h}`, expected `Name: value`")))?;
        let name = name.trim();
        if name.is_empty() {
            return err(format!("invalid header `{h}`, the name is empty"));
        }
        req.headers.retain(|e| !e.name.eq_ignore_ascii_case(name));
        req.headers.push(KeyValue::new(name, value.trim()));
    }
    for q in &args.query {
        let (name, value) = q.split_once('=').unwrap_or((q, ""));
        req.query.push(KeyValue::new(name, value));
    }
    let body = body(args)?;
    if !body.is_none() {
        req.body = body;
    }
    if let Some(auth) = auth(args)? {
        req.auth = auth;
    }

    let s = &mut req.settings;
    if let Some(t) = args.timeout {
        s.timeout = seconds(t, "--timeout")?;
    }
    if let Some(t) = args.connect_timeout {
        s.connect_timeout = seconds(t, "--connect-timeout")?;
    }
    if args.http1 {
        s.http_version = HttpVersionPref::Http1;
    } else if args.http2 {
        s.http_version = HttpVersionPref::Http2;
    }
    if let Some(p) = &args.proxy {
        s.proxy = ProxyMode::try_from(p.clone()).map_err(ArgError)?;
    }
    for r in &args.resolve {
        s.resolve
            .push(r.parse::<ResolveOverride>().map_err(ArgError)?);
    }
    if args.no_follow {
        s.follow_redirects = false;
    }
    if let Some(n) = args.max_redirs {
        s.max_redirects = n;
    }
    if args.no_decompress {
        s.decompress = false;
    }
    if args.no_cookies {
        s.cookies = false;
    }
    if args.insecure {
        s.tls.verify = false;
    }
    if let Some(ca) = &args.cacert {
        s.tls.ca_cert = Some(ca.clone());
    }
    if let (Some(cert), Some(key)) = (&args.cert, &args.key) {
        s.tls.client_cert = Some(ClientCertificate {
            cert: cert.clone(),
            key: key.clone(),
        });
    }
    if let Some(v) = args.tls_min {
        s.tls.min_version = Some(tls_version(v));
    }
    if let Some(v) = args.tls_max {
        s.tls.max_version = Some(tls_version(v));
    }
    if args.verbose >= 2 {
        s.capture_wire = true;
    }
    for secret in &args.secrets {
        if !s.secrets.contains(secret) {
            s.secrets.push(secret.clone());
        }
    }
    Ok(())
}

/// The auth given on the command line, if any.
fn auth(args: &SendArgs) -> Result<Option<Auth>, ArgError> {
    if let Some(user) = &args.user {
        let (username, password) = user.split_once(':').unwrap_or((user, ""));
        return Ok(Some(Auth::Basic {
            username: username.to_owned(),
            password: password.to_owned(),
        }));
    }
    if let Some(token) = &args.bearer {
        if token.trim().is_empty() {
            return err("--bearer: the token is empty");
        }
        return Ok(Some(Auth::Bearer {
            token: token.trim().to_owned(),
        }));
    }
    if let Some(key) = &args.api_key {
        let Some((name, value)) = key.split_once('=').filter(|(n, _)| !n.trim().is_empty()) else {
            return err(format!(
                "invalid --api-key `{}`, expected `NAME=VALUE`",
                key.split_once('=').map_or(key.as_str(), |(n, _)| n)
            ));
        };
        return Ok(Some(Auth::ApiKey {
            name: name.trim().to_owned(),
            value: value.to_owned(),
            location: match args.api_key_in {
                ApiKeyInArg::Header => ApiKeyLocation::Header,
                ApiKeyInArg::Query => ApiKeyLocation::Query,
            },
        }));
    }
    Ok(args.no_auth.then_some(Auth::None))
}

/// `-u USER` without a password: whether to ask for one.
pub(crate) fn needs_password_prompt(args: &SendArgs) -> bool {
    args.user.as_deref().is_some_and(|u| !u.contains(':'))
}

fn body(args: &SendArgs) -> Result<Body, ArgError> {
    if let Some(data) = &args.data {
        return Ok(match data.strip_prefix('@') {
            Some(path) => Body::File {
                path: PathBuf::from(path),
                content_type: None,
            },
            None => Body::Text {
                content: data.clone(),
                content_type: None,
            },
        });
    }
    if let Some(json) = &args.json {
        return Ok(match json.strip_prefix('@') {
            Some(path) => Body::File {
                path: PathBuf::from(path),
                content_type: Some("application/json".into()),
            },
            None => Body::Json {
                content: json.clone(),
            },
        });
    }
    if !args.form.is_empty() {
        let fields = args
            .form
            .iter()
            .map(|f| {
                let (n, v) = f.split_once('=').unwrap_or((f, ""));
                KeyValue::new(n, v)
            })
            .collect();
        return Ok(Body::Form { fields });
    }
    if !args.multipart.is_empty() {
        let parts = args
            .multipart
            .iter()
            .map(|f| multipart_part(f))
            .collect::<Result<_, _>>()?;
        return Ok(Body::Multipart { parts });
    }
    if let Some(path) = &args.file {
        return Ok(Body::File {
            path: path.clone(),
            content_type: None,
        });
    }
    Ok(Body::None)
}

/// `name=value`, `name=@path` or `name=@path;type=mime`.
fn multipart_part(field: &str) -> Result<MultipartPart, ArgError> {
    let (name, value) = field.split_once('=').ok_or_else(|| {
        ArgError(format!(
            "invalid multipart field `{field}`, expected `name=value` or `name=@path`"
        ))
    })?;
    let value = match value.strip_prefix('@') {
        Some(file) => {
            let (path, content_type) = match file.split_once(";type=") {
                Some((p, t)) => (p, Some(t.to_owned())),
                None => (file, None),
            };
            PartValue::File {
                path: PathBuf::from(path),
                filename: None,
                content_type,
            }
        }
        None => PartValue::Text {
            value: value.to_owned(),
        },
    };
    Ok(MultipartPart {
        name: name.to_owned(),
        value,
        enabled: true,
    })
}

/// Like curl, a URL without a scheme means `http://`.
fn with_default_scheme(url: &str) -> String {
    if url.contains("://") {
        url.to_owned()
    } else {
        format!("http://{url}")
    }
}

/// Seconds as a duration; `0` means no limit.
fn seconds(value: f64, flag: &str) -> Result<Option<Duration>, ArgError> {
    if !value.is_finite() || value < 0.0 {
        return err(format!("{flag} must be a number of seconds, 0 or more"));
    }
    Ok((value > 0.0).then(|| Duration::from_secs_f64(value)))
}

fn tls_version(v: TlsVersionArg) -> TlsVersion {
    match v {
        TlsVersionArg::Tls12 => TlsVersion::Tls12,
        TlsVersionArg::Tls13 => TlsVersion::Tls13,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Command};
    use clap::Parser;

    fn parse(args: &[&str]) -> Result<Request, ArgError> {
        let cli = Cli::try_parse_from(["snapreq", "send"].iter().chain(args)).unwrap();
        let Command::Send(send) = cli.command else {
            panic!("expected the send command");
        };
        build_request(&send)
    }

    #[test]
    fn method_inference_and_default_scheme() {
        let r = parse(&["example.com/x"]).unwrap();
        assert_eq!(
            (r.method.as_str(), r.url.as_str()),
            ("GET", "http://example.com/x")
        );

        let r = parse(&["https://a.test", "-d", "hi"]).unwrap();
        assert_eq!(r.method, Method::POST);

        let r = parse(&["delete", "https://a.test"]).unwrap();
        assert_eq!(r.method, Method::DELETE);
        assert!(parse(&["G T", "https://a.test"]).is_err());
    }

    #[test]
    fn headers_and_query() {
        let r = parse(&[
            "https://a.test",
            "-H",
            "Accept: application/json",
            "-H",
            "X-Empty:",
            "-q",
            "page=2",
            "-q",
            "flag",
        ])
        .unwrap();
        let headers: Vec<_> = r
            .headers
            .iter()
            .map(|h| (h.name.as_str(), h.value.as_str()))
            .collect();
        assert_eq!(headers, [("Accept", "application/json"), ("X-Empty", "")]);
        let query: Vec<_> = r
            .query
            .iter()
            .map(|q| (q.name.as_str(), q.value.as_str()))
            .collect();
        assert_eq!(query, [("page", "2"), ("flag", "")]);

        assert_eq!(
            parse(&["https://a.test", "-H", "NoColon"]).unwrap_err().0,
            "invalid header `NoColon`, expected `Name: value`"
        );
    }

    #[test]
    fn bodies() {
        assert_eq!(
            parse(&["https://a.test", "--json", r#"{"a":1}"#])
                .unwrap()
                .body,
            Body::Json {
                content: r#"{"a":1}"#.into()
            }
        );
        assert_eq!(
            parse(&["https://a.test", "--json", "@data.json"])
                .unwrap()
                .body,
            Body::File {
                path: "data.json".into(),
                content_type: Some("application/json".into())
            }
        );
        assert_eq!(
            parse(&["https://a.test", "-d", "@raw.bin"]).unwrap().body,
            Body::File {
                path: "raw.bin".into(),
                content_type: None
            }
        );
        assert_eq!(
            parse(&["https://a.test", "--form", "a=1", "--form", "b=x y"])
                .unwrap()
                .body,
            Body::Form {
                fields: vec![KeyValue::new("a", "1"), KeyValue::new("b", "x y")]
            }
        );
        let Body::Multipart { parts } = parse(&[
            "https://a.test",
            "-F",
            "title=Hi",
            "-F",
            "img=@me.png;type=image/png",
        ])
        .unwrap()
        .body
        else {
            panic!("expected multipart");
        };
        assert_eq!(parts[0].value, PartValue::Text { value: "Hi".into() });
        assert_eq!(
            parts[1].value,
            PartValue::File {
                path: "me.png".into(),
                filename: None,
                content_type: Some("image/png".into())
            }
        );
    }

    #[test]
    fn only_one_body_kind() {
        let res = Cli::try_parse_from([
            "snapreq",
            "send",
            "https://a.test",
            "-d",
            "x",
            "--json",
            "{}",
        ]);
        assert!(res.is_err());
    }

    #[test]
    fn settings() {
        let r = parse(&[
            "https://a.test",
            "-t",
            "2.5",
            "--connect-timeout",
            "0",
            "--http2",
            "-x",
            "socks5h://p:1080",
            "--resolve",
            "a.test:443:127.0.0.1",
            "--no-follow",
            "--max-redirs",
            "3",
            "--no-decompress",
            "--no-cookies",
            "-k",
            "--cacert",
            "ca.pem",
            "--cert",
            "me.crt",
            "--key",
            "me.key",
            "--tls-min",
            "1.3",
            "-vv",
            "--secret",
            "tok",
        ])
        .unwrap();
        let s = &r.settings;
        assert_eq!(s.timeout, Some(Duration::from_millis(2500)));
        assert_eq!(s.connect_timeout, None);
        assert_eq!(s.http_version, HttpVersionPref::Http2);
        assert_eq!(s.proxy, ProxyMode::Url("socks5h://p:1080".into()));
        assert_eq!(s.resolve.len(), 1);
        assert!(!s.follow_redirects && !s.decompress && !s.cookies && !s.tls.verify);
        assert_eq!(s.max_redirects, 3);
        assert_eq!(s.tls.ca_cert, Some("ca.pem".into()));
        assert!(s.tls.client_cert.is_some());
        assert_eq!(s.tls.min_version, Some(TlsVersion::Tls13));
        assert!(s.capture_wire);
        assert_eq!(s.secrets, ["tok"]);
    }

    #[test]
    fn auth_options() {
        use snapreq_core::model::{ApiKeyLocation, Auth};
        let auth = |args: &[&str]| parse(&[&["http://a.test"], args].concat()).map(|r| r.auth);
        assert_eq!(auth(&[]).unwrap(), Auth::None);
        assert_eq!(
            auth(&["-u", "me:p:w"]).unwrap(),
            Auth::Basic {
                username: "me".into(),
                password: "p:w".into()
            }
        );
        assert_eq!(
            auth(&["--user", "me"]).unwrap(),
            Auth::Basic {
                username: "me".into(),
                password: String::new()
            }
        );
        assert_eq!(
            auth(&["--bearer", " t "]).unwrap(),
            Auth::Bearer { token: "t".into() }
        );
        assert_eq!(
            auth(&["--api-key", "X-Key=a=b"]).unwrap(),
            Auth::ApiKey {
                name: "X-Key".into(),
                value: "a=b".into(),
                location: ApiKeyLocation::Header
            }
        );
        assert_eq!(
            auth(&["--api-key", "k=v", "--api-key-in", "query"]).unwrap(),
            Auth::ApiKey {
                name: "k".into(),
                value: "v".into(),
                location: ApiKeyLocation::Query
            }
        );
        assert!(
            auth(&["--api-key", "novalue"])
                .unwrap_err()
                .0
                .contains("NAME=VALUE")
        );
        assert!(auth(&["--api-key", "=v"]).is_err());
        assert!(auth(&["--bearer", " "]).is_err());
    }

    #[test]
    fn existing_auth_is_kept_unless_replaced() {
        use snapreq_core::model::Auth;
        let cli = Cli::try_parse_from(["snapreq", "send", "x", "--no-auth"]).unwrap();
        let Command::Send(args) = cli.command else {
            panic!()
        };
        let mut req = Request::get("http://a.test").auth(Auth::Bearer { token: "t".into() });
        apply_overrides(&mut req, &args).unwrap();
        assert_eq!(req.auth, Auth::None);

        let cli = Cli::try_parse_from(["snapreq", "send", "x"]).unwrap();
        let Command::Send(args) = cli.command else {
            panic!()
        };
        let mut req = Request::get("http://a.test").auth(Auth::Bearer { token: "t".into() });
        apply_overrides(&mut req, &args).unwrap();
        assert_eq!(req.auth, Auth::Bearer { token: "t".into() });
        assert!(needs_password_prompt(&match Cli::try_parse_from([
            "snapreq", "send", "x", "-u", "me"
        ])
        .unwrap()
        .command
        {
            Command::Send(a) => a,
            _ => panic!(),
        }));
    }

    #[test]
    fn auth_options_conflict() {
        for args in [
            &["-u", "a:b", "--bearer", "t"][..],
            &["--bearer", "t", "--api-key", "k=v"],
            &["--api-key", "k=v", "--no-auth"],
            &["--api-key-in", "query"],
        ] {
            let full: Vec<&str> = ["snapreq", "send", "x"]
                .iter()
                .copied()
                .chain(args.iter().copied())
                .collect();
            assert!(Cli::try_parse_from(full).is_err(), "{args:?}");
        }
    }

    #[test]
    fn invalid_settings() {
        assert!(parse(&["https://a.test", "-t", "-1"]).is_err());
        assert!(parse(&["https://a.test", "--resolve", "nope"]).is_err());
        assert!(parse(&["https://a.test", "-x", "ftp://p"]).is_err());
        assert!(
            Cli::try_parse_from(["snapreq", "send", "https://a.test", "--cert", "c.pem"]).is_err()
        );
        assert!(Cli::try_parse_from(["snapreq", "send", "u", "--http1.1", "--http2"]).is_err());
    }
}
