//! HTTP Digest authentication (RFC 7616, with RFC 2069 compatibility).
//!
//! Supported: algorithms `MD5`, `SHA-256`, `SHA-512-256` and their `-sess` variants,
//! `qop=auth` (or no qop), `userhash`, and UTF-8 user names (`username*`).
//! Not supported: `qop=auth-int` without `auth` (it would need the whole body hashed).

use std::fmt::Write as _;

use md5::Digest as _;

/// One challenge from a `WWW-Authenticate` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Challenge {
    /// Lowercase scheme, for example `digest`.
    pub scheme: String,
    /// Parameters with lowercase names, in order.
    pub params: Vec<(String, String)>,
}

impl Challenge {
    fn param(&self, name: &str) -> Option<&str> {
        self.params
            .iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }
}

/// Parses every challenge in `values` (all `WWW-Authenticate` header values).
/// A single header may hold several challenges: `Basic realm="a", Digest realm="b", nonce="c"`.
pub(crate) fn parse_challenges<'a>(values: impl IntoIterator<Item = &'a str>) -> Vec<Challenge> {
    let mut out: Vec<Challenge> = Vec::new();
    for value in values {
        let mut p = Parser {
            s: value.as_bytes(),
            i: 0,
        };
        // Parameters before any scheme are ignored.
        let mut current: Option<Challenge> = None;
        loop {
            p.skip(|c| c == b' ' || c == b'\t' || c == b',');
            if p.done() {
                break;
            }
            let token = p.token();
            if token.is_empty() {
                // Not a token character: skip it and resynchronize.
                p.i += 1;
                continue;
            }
            p.skip(|c| c == b' ' || c == b'\t');
            if p.peek() == Some(b'=') {
                p.i += 1;
                p.skip(|c| c == b' ' || c == b'\t');
                let value = if p.peek() == Some(b'"') {
                    p.quoted()
                } else {
                    p.token()
                };
                if let Some(c) = current.as_mut() {
                    c.params.push((token.to_ascii_lowercase(), value));
                }
            } else {
                if let Some(c) = current.take() {
                    out.push(c);
                }
                current = Some(Challenge {
                    scheme: token.to_ascii_lowercase(),
                    params: Vec::new(),
                });
                // A token68 credential (`Negotiate abc==`) is not used by Digest; skip it.
                let save = p.i;
                let t68 = p.token68();
                p.skip(|c| c == b' ' || c == b'\t');
                if t68.is_empty() || !matches!(p.peek(), None | Some(b',')) {
                    p.i = save;
                }
            }
        }
        if let Some(c) = current {
            out.push(c);
        }
    }
    out
}

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn done(&self) -> bool {
        self.i >= self.s.len()
    }

    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }

    fn skip(&mut self, f: impl Fn(u8) -> bool) {
        while self.peek().is_some_and(&f) {
            self.i += 1;
        }
    }

    fn take(&mut self, f: impl Fn(u8) -> bool) -> String {
        let start = self.i;
        self.skip(f);
        String::from_utf8_lossy(&self.s[start..self.i]).into_owned()
    }

    /// RFC 9110 `token`.
    fn token(&mut self) -> String {
        self.take(|c| c.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&c))
    }

    fn token68(&mut self) -> String {
        let mut t = self.take(|c| c.is_ascii_alphanumeric() || b"-._~+/".contains(&c));
        if !t.is_empty() {
            t.push_str(&self.take(|c| c == b'='));
        }
        t
    }

    /// A quoted string starting at `"`; backslash escapes are undone.
    fn quoted(&mut self) -> String {
        self.i += 1;
        let mut out = Vec::new();
        while let Some(c) = self.peek() {
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    if let Some(next) = self.peek() {
                        out.push(next);
                        self.i += 1;
                    }
                }
                _ => out.push(c),
            }
        }
        String::from_utf8_lossy(&out).into_owned()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Algorithm {
    Md5,
    Sha256,
    Sha512_256,
}

impl Algorithm {
    /// Parses `MD5`, `SHA-256-sess` and so on; returns the algorithm and whether it is a
    /// session variant.
    fn parse(name: &str) -> Option<(Self, bool)> {
        let upper = name.trim().to_ascii_uppercase();
        let (base, sess) = match upper.strip_suffix("-SESS") {
            Some(base) => (base, true),
            None => (upper.as_str(), false),
        };
        let alg = match base {
            "MD5" => Self::Md5,
            "SHA-256" => Self::Sha256,
            "SHA-512-256" => Self::Sha512_256,
            _ => return None,
        };
        Some((alg, sess))
    }

    fn hash(self, data: &str) -> String {
        match self {
            Self::Md5 => hex(&md5::Md5::digest(data.as_bytes())),
            Self::Sha256 => {
                hex(ring::digest::digest(&ring::digest::SHA256, data.as_bytes()).as_ref())
            }
            Self::Sha512_256 => {
                hex(ring::digest::digest(&ring::digest::SHA512_256, data.as_bytes()).as_ref())
            }
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Why no digest response could be computed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DigestError {
    /// The response has no Digest challenge at all.
    NoChallenge,
    /// Digest challenges exist but none can be answered; the message says why.
    Unsupported(String),
}

/// What is needed to answer a challenge.
pub(crate) struct Credentials<'a> {
    pub username: &'a str,
    pub password: &'a str,
    pub method: &'a str,
    /// The request target, `/path?query`.
    pub uri: &'a str,
    /// Client nonce; random in real use, fixed in tests.
    pub cnonce: &'a str,
}

/// Picks the first Digest challenge in `challenges` that can be answered and returns the
/// `Authorization` header value for it.
pub(crate) fn authorization(
    challenges: &[Challenge],
    creds: &Credentials<'_>,
) -> Result<String, DigestError> {
    let digests: Vec<&Challenge> = challenges.iter().filter(|c| c.scheme == "digest").collect();
    if digests.is_empty() {
        return Err(DigestError::NoChallenge);
    }
    let mut reasons = Vec::new();
    for c in digests {
        match answer(c, creds) {
            Ok(v) => return Ok(v),
            Err(why) => reasons.push(why),
        }
    }
    Err(DigestError::Unsupported(reasons.join("; ")))
}

fn answer(c: &Challenge, creds: &Credentials<'_>) -> Result<String, String> {
    let realm = c.param("realm").unwrap_or_default();
    let nonce = c
        .param("nonce")
        .ok_or_else(|| "challenge without a nonce".to_owned())?;
    let algorithm_name = c.param("algorithm").unwrap_or("MD5");
    let (alg, sess) = Algorithm::parse(algorithm_name)
        .ok_or_else(|| format!("unsupported algorithm `{algorithm_name}`"))?;
    let qop = match c.param("qop") {
        None => None,
        Some(list) => {
            let offered: Vec<String> = list
                .split(',')
                .map(|q| q.trim().to_ascii_lowercase())
                .collect();
            if !offered.iter().any(|q| q == "auth") {
                return Err(format!(
                    "unsupported qop `{list}` (only `auth` is supported)"
                ));
            }
            Some("auth")
        }
    };
    let nc = "00000001";

    let mut ha1 = alg.hash(&format!("{}:{realm}:{}", creds.username, creds.password));
    if sess {
        ha1 = alg.hash(&format!("{ha1}:{nonce}:{}", creds.cnonce));
    }
    let ha2 = alg.hash(&format!("{}:{}", creds.method, creds.uri));
    let response = match qop {
        Some(q) => alg.hash(&format!("{ha1}:{nonce}:{nc}:{}:{q}:{ha2}", creds.cnonce)),
        None => alg.hash(&format!("{ha1}:{nonce}:{ha2}")),
    };

    let userhash = c
        .param("userhash")
        .is_some_and(|v| v.eq_ignore_ascii_case("true"));
    let mut out = String::from("Digest ");
    if userhash {
        let _ = write!(
            out,
            "username=\"{}\", userhash=true",
            alg.hash(&format!("{}:{realm}", creds.username))
        );
    } else if needs_extended(creds.username) {
        // RFC 7616 section 3.4.4: RFC 8187 encoding for names that cannot be quoted.
        let encoded = percent_encoding::utf8_percent_encode(
            creds.username,
            percent_encoding::NON_ALPHANUMERIC,
        );
        let _ = write!(out, "username*=UTF-8''{encoded}");
    } else {
        let _ = write!(out, "username=\"{}\"", creds.username);
    }
    let _ = write!(
        out,
        ", realm=\"{}\", nonce=\"{}\", uri=\"{}\", algorithm={algorithm_name}, response=\"{response}\"",
        quote(realm),
        quote(nonce),
        quote(creds.uri)
    );
    if let Some(q) = qop {
        let _ = write!(
            out,
            ", qop={q}, nc={nc}, cnonce=\"{}\"",
            quote(creds.cnonce)
        );
    }
    if let Some(opaque) = c.param("opaque") {
        let _ = write!(out, ", opaque=\"{}\"", quote(opaque));
    }
    Ok(out)
}

fn needs_extended(name: &str) -> bool {
    name.bytes()
        .any(|b| !(0x20..0x7f).contains(&b) || b == b'"' || b == b'\\')
}

fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// A random client nonce.
pub(crate) fn cnonce() -> String {
    use ring::rand::SecureRandom as _;
    let mut bytes = [0u8; 16];
    // The system RNG does not fail on supported platforms; a fixed fallback would still
    // produce a valid (if predictable) nonce.
    let _ = ring::rand::SystemRandom::new().fill(&mut bytes);
    hex(&bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn creds<'a>(
        username: &'a str,
        method: &'a str,
        uri: &'a str,
        cnonce: &'a str,
    ) -> Credentials<'a> {
        Credentials {
            username,
            password: "Circle of Life",
            method,
            uri,
            cnonce,
        }
    }

    /// The value of `name=` in a header we built.
    fn field<'a>(header: &'a str, name: &str) -> &'a str {
        let start = header.find(&format!("{name}=")).unwrap() + name.len() + 1;
        let rest = &header[start..];
        if let Some(q) = rest.strip_prefix('"') {
            &q[..q.find('"').unwrap()]
        } else {
            &rest[..rest.find(',').unwrap_or(rest.len())]
        }
    }

    #[test]
    fn parses_several_challenges_in_one_header() {
        let c = parse_challenges([
            r#"Basic realm="simple", Digest realm="http-auth@example.org", qop="auth, auth-int", algorithm=SHA-256, nonce="7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v", opaque="FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS""#,
            r#"Negotiate abc==, Digest realm="a \"quoted\" realm", nonce=n2"#,
        ]);
        let schemes: Vec<&str> = c.iter().map(|c| c.scheme.as_str()).collect();
        assert_eq!(schemes, ["basic", "digest", "negotiate", "digest"]);
        assert_eq!(c[0].param("realm"), Some("simple"));
        assert_eq!(c[1].param("qop"), Some("auth, auth-int"));
        assert_eq!(c[1].param("algorithm"), Some("SHA-256"));
        assert!(c[2].params.is_empty());
        assert_eq!(c[3].param("realm"), Some(r#"a "quoted" realm"#));
        assert_eq!(c[3].param("nonce"), Some("n2"));
        assert!(parse_challenges(["", " , ,"]).is_empty());
    }

    /// RFC 7616 section 3.9.1, MD5 and SHA-256 examples.
    #[test]
    fn rfc_7616_examples() {
        let header = |alg: &str| {
            format!(
                r#"Digest realm="http-auth@example.org", qop="auth, auth-int", algorithm={alg}, nonce="7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v", opaque="FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS""#
            )
        };
        let cnonce = "f2/wE4q74E6zIJEtWaHKaf5wv/H5QzzpXusqGemxURZJ";
        let c = creds("Mufasa", "GET", "/dir/index.html", cnonce);

        let md5 = authorization(&parse_challenges([header("MD5").as_str()]), &c).unwrap();
        assert_eq!(field(&md5, "response"), "8ca523f5e9506fed4657c9700eebdbec");
        let sha = authorization(&parse_challenges([header("SHA-256").as_str()]), &c).unwrap();
        assert_eq!(
            field(&sha, "response"),
            "753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1"
        );
        assert_eq!(
            sha,
            format!(
                r#"Digest username="Mufasa", realm="http-auth@example.org", nonce="7ypf/xlj9XXwfDPEoM4URrv/xwf94BcCAzFZH4GiTo0v", uri="/dir/index.html", algorithm=SHA-256, response="753927fa0e85d155564e2e272a28d1802ca10daf4496794697cf8db5856cb6c1", qop=auth, nc=00000001, cnonce="{cnonce}", opaque="FQhe/qaU925kfnzjCev0ciny7QMkPqMAFRtzCUYo5tdS""#
            )
        );
    }

    /// RFC 2617 section 3.5 (MD5, qop=auth) and RFC 2069 style (no qop).
    #[test]
    fn rfc_2617_example_and_no_qop() {
        let c = parse_challenges([
            r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        ]);
        let mut cr = creds("Mufasa", "GET", "/dir/index.html", "0a4f113b");
        // RFC 2617 spells the password with a capital O.
        cr.password = "Circle Of Life";
        assert_eq!(
            field(&authorization(&c, &cr).unwrap(), "response"),
            "6629fae49393a05397450978507c4ef1"
        );

        let no_qop = parse_challenges([r#"Digest realm="r", nonce="n""#]);
        cr.password = "p";
        let h = authorization(&no_qop, &cr).unwrap();
        let ha1 = Algorithm::Md5.hash("Mufasa:r:p");
        let ha2 = Algorithm::Md5.hash("GET:/dir/index.html");
        assert_eq!(
            field(&h, "response"),
            Algorithm::Md5.hash(&format!("{ha1}:n:{ha2}"))
        );
        assert!(!h.contains("qop=") && !h.contains("cnonce"));
    }

    #[test]
    fn session_variants_userhash_and_utf8_names() {
        let c = parse_challenges([
            r#"Digest realm="r", nonce="n", qop=auth, algorithm=SHA-256-sess, userhash=true"#,
        ]);
        let cr = creds("Mufasa", "POST", "/x?y=1", "cn");
        let h = authorization(&c, &cr).unwrap();
        let alg = Algorithm::Sha256;
        let ha1 = alg.hash(&format!("{}:n:cn", alg.hash("Mufasa:r:Circle of Life")));
        let ha2 = alg.hash("POST:/x?y=1");
        assert_eq!(
            field(&h, "response"),
            alg.hash(&format!("{ha1}:n:00000001:cn:auth:{ha2}"))
        );
        assert_eq!(field(&h, "username"), alg.hash("Mufasa:r"));
        assert!(h.contains("userhash=true"));

        // RFC 7616 section 3.9.2: the user name in RFC 8187 encoding.
        let c = parse_challenges([
            r#"Digest realm="api@example.org", nonce="n", qop=auth, algorithm=SHA-512-256"#,
        ]);
        let h = authorization(&c, &creds("J\u{e4}s\u{f8}n Doe", "GET", "/", "cn")).unwrap();
        assert!(
            h.starts_with("Digest username*=UTF-8''J%C3%A4s%C3%B8n%20Doe, "),
            "{h}"
        );
    }

    #[test]
    fn unsupported_challenges_say_why() {
        let cr = creds("u", "GET", "/", "cn");
        assert_eq!(
            authorization(&parse_challenges([r#"Basic realm="x""#]), &cr),
            Err(DigestError::NoChallenge)
        );
        let err = authorization(
            &parse_challenges([
                r#"Digest realm="r", nonce="n", algorithm=SHA-1"#,
                r#"Digest realm="r", nonce="n", qop="auth-int""#,
            ]),
            &cr,
        )
        .unwrap_err();
        assert_eq!(
            err,
            DigestError::Unsupported(
                "unsupported algorithm `SHA-1`; unsupported qop `auth-int` (only `auth` is supported)".into()
            )
        );
        // The first supported challenge wins.
        let h = authorization(
            &parse_challenges([
                r#"Digest realm="r", nonce="n", algorithm=SHA-1"#,
                r#"Digest realm="r", nonce="n", algorithm=SHA-256"#,
            ]),
            &cr,
        )
        .unwrap();
        assert!(h.contains("algorithm=SHA-256"));
        assert_eq!(cnonce().len(), 32);
        assert_ne!(cnonce(), cnonce());
    }
}
