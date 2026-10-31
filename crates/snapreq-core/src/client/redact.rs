//! Redaction of sensitive data in traces.
//!
//! Only the trace (the debug log that is printed, stored and shared) is redacted.
//! What is sent on the wire and the returned response itself are never changed.

use bytes::Bytes;

use crate::model::{Header, TraceEntry, TraceEvent};

/// Replacement text for hidden values.
pub(crate) const REDACTED: &str = "<redacted>";

/// Headers redacted by default.
pub const DEFAULT_SENSITIVE_HEADERS: [&str; 7] = [
    "authorization",
    "proxy-authorization",
    "cookie",
    "set-cookie",
    "x-api-key",
    "api-key",
    "x-auth-token",
];

#[derive(Debug, Clone, Default)]
pub(crate) struct Redactor {
    /// Lowercase header names.
    headers: Vec<String>,
    /// Exact values to hide wherever they appear.
    secrets: Vec<String>,
}

impl Redactor {
    pub(crate) fn new(headers: &[String], secrets: &[String]) -> Self {
        Self {
            headers: headers
                .iter()
                .map(|h| h.trim().to_ascii_lowercase())
                .collect(),
            // Longest first, so a secret that contains another is hidden whole.
            secrets: {
                let mut s: Vec<String> =
                    secrets.iter().filter(|s| !s.is_empty()).cloned().collect();
                s.sort_by_key(|s| std::cmp::Reverse(s.len()));
                s
            },
        }
    }

    /// Hides `secret` from now on. Values derived automatically are only added when
    /// they are long enough not to blank out unrelated text.
    pub(crate) fn add_secret(&mut self, secret: &str) {
        if secret.is_empty() || self.secrets.iter().any(|s| s == secret) {
            return;
        }
        self.secrets.push(secret.to_owned());
        self.secrets.sort_by_key(|s| std::cmp::Reverse(s.len()));
    }

    /// Learns the credential values inside sensitive request headers, so they are
    /// hidden even where a server echoes them back (for example in a response body).
    pub(crate) fn learn_from_header(&mut self, name: &str, value: &str) {
        const MIN_LEN: usize = 6;
        if !self.is_sensitive(name) {
            return;
        }
        let lower = name.trim().to_ascii_lowercase();
        // Only the credential itself, so the trace still shows the auth scheme and cookie names.
        let candidates: Vec<&str> = match lower.as_str() {
            "authorization" | "proxy-authorization" => match value.trim().split_once(' ') {
                Some((_, credential)) => vec![credential.trim()],
                None => vec![value.trim()],
            },
            "cookie" => value
                .split(';')
                .filter_map(|pair| pair.split_once('=').map(|(_, v)| v.trim()))
                .collect(),
            _ => vec![value.trim()],
        };
        for c in candidates {
            if c.len() >= MIN_LEN {
                self.add_secret(c);
            }
        }
    }

    fn is_sensitive(&self, name: &str) -> bool {
        self.headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case(name.trim()))
    }

    /// Hides secret values inside free text.
    pub(crate) fn text(&self, s: &str) -> String {
        let mut out = s.to_owned();
        for secret in &self.secrets {
            if out.contains(secret.as_str()) {
                out = out.replace(secret.as_str(), REDACTED);
            }
        }
        out
    }

    /// Redacts one header value, keeping structure that helps debugging:
    /// the auth scheme, cookie names and `Set-Cookie` attributes.
    pub(crate) fn header_value(&self, name: &str, value: &str) -> String {
        if !self.is_sensitive(name) {
            return self.text(value);
        }
        let lower = name.trim().to_ascii_lowercase();
        match lower.as_str() {
            "authorization" | "proxy-authorization" => match value.trim().split_once(' ') {
                Some((scheme, _)) => format!("{scheme} {REDACTED}"),
                None => REDACTED.to_owned(),
            },
            "cookie" => value
                .split(';')
                .map(|pair| match pair.split_once('=') {
                    Some((n, _)) => format!("{}={REDACTED}", n.trim()),
                    None => REDACTED.to_owned(),
                })
                .collect::<Vec<_>>()
                .join("; "),
            "set-cookie" => {
                let (first, attrs) = value.split_once(';').unwrap_or((value, ""));
                let name = first.split_once('=').map_or("", |(n, _)| n.trim());
                if attrs.is_empty() {
                    format!("{name}={REDACTED}")
                } else {
                    format!("{name}={REDACTED};{attrs}")
                }
            }
            _ => REDACTED.to_owned(),
        }
    }

    fn headers(&self, headers: &mut [Header]) {
        for h in headers {
            h.value = self.header_value(&h.name, &h.value);
        }
    }

    /// Redacts one captured HTTP/1.1 message: sensitive header lines in the head,
    /// secrets everywhere. Binary content after the head is kept byte-for-byte.
    pub(crate) fn wire(&self, data: &Bytes) -> Bytes {
        let head_end = data
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map_or(data.len(), |p| p + 4);
        let mut out = String::with_capacity(data.len());
        for line in String::from_utf8_lossy(&data[..head_end]).split_inclusive('\n') {
            let content = line.trim_end_matches(['\r', '\n']);
            let ending = &line[content.len()..];
            match content.split_once(':') {
                Some((name, value)) if !name.is_empty() && !name.contains(' ') => {
                    out.push_str(name);
                    out.push_str(": ");
                    out.push_str(&self.header_value(name, value.trim()));
                    out.push_str(ending);
                }
                _ => out.push_str(&self.text(line)),
            }
        }
        let mut bytes = out.into_bytes();
        let body = &data[head_end..];
        match std::str::from_utf8(body) {
            Ok(text) => bytes.extend_from_slice(self.text(text).as_bytes()),
            Err(_) => bytes.extend_from_slice(body),
        }
        Bytes::from(bytes)
    }

    /// Redacts every event in a trace in place.
    pub(crate) fn trace(&self, entries: &mut [TraceEntry]) {
        if self.headers.is_empty() && self.secrets.is_empty() {
            return;
        }
        for entry in entries {
            match &mut entry.event {
                TraceEvent::RequestHeaders { url, headers, .. } => {
                    *url = self.text(url);
                    self.headers(headers);
                }
                TraceEvent::ResponseHeaders { headers, .. } => self.headers(headers),
                TraceEvent::Redirect { from, to, .. } => {
                    *from = self.text(from);
                    *to = self.text(to);
                }
                TraceEvent::Info { message } | TraceEvent::Error { message, .. } => {
                    *message = self.text(message);
                }
                TraceEvent::Wire { sent, received, .. } => {
                    *sent = self.wire(sent);
                    *received = self.wire(received);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor(secrets: &[&str]) -> Redactor {
        let headers: Vec<String> = DEFAULT_SENSITIVE_HEADERS
            .iter()
            .map(|h| (*h).to_owned())
            .collect();
        let secrets: Vec<String> = secrets.iter().map(|s| (*s).to_owned()).collect();
        Redactor::new(&headers, &secrets)
    }

    #[test]
    fn header_values_keep_useful_structure() {
        let r = redactor(&[]);
        assert_eq!(
            r.header_value("Authorization", "Bearer abc.def"),
            "Bearer <redacted>"
        );
        assert_eq!(r.header_value("authorization", "opaque"), "<redacted>");
        assert_eq!(
            r.header_value("Cookie", "a=1; b=2"),
            "a=<redacted>; b=<redacted>"
        );
        assert_eq!(
            r.header_value("Set-Cookie", "sid=xyz; Path=/; HttpOnly"),
            "sid=<redacted>; Path=/; HttpOnly"
        );
        assert_eq!(r.header_value("X-Api-Key", "k"), "<redacted>");
        assert_eq!(r.header_value("Accept", "*/*"), "*/*");
    }

    #[test]
    fn secrets_are_hidden_everywhere_longest_first() {
        let r = redactor(&["tok", "tok-123"]);
        assert_eq!(
            r.text("https://a.test/?key=tok-123&x=tok"),
            "https://a.test/?key=<redacted>&x=<redacted>"
        );
        assert_eq!(r.header_value("X-Custom", "pre-tok-123"), "pre-<redacted>");
    }

    #[test]
    fn credentials_in_sensitive_headers_become_secrets() {
        let mut r = redactor(&[]);
        r.learn_from_header("Authorization", "Bearer abc.def.ghi");
        r.learn_from_header("Cookie", "sid=session-123; n=1");
        r.learn_from_header("Accept", "application/json");
        assert_eq!(
            r.text("echo: Bearer abc.def.ghi, session-123, n=1, application/json"),
            "echo: Bearer <redacted>, <redacted>, n=1, application/json"
        );
    }

    #[test]
    fn wire_heads_and_bodies() {
        let r = redactor(&["s3cr3t"]);
        let wire = Bytes::from_static(
            b"POST /login HTTP/1.1\r\nHost: a.test\r\nAuthorization: Basic dXNlcg==\r\n\r\n{\"password\":\"s3cr3t\"}",
        );
        assert_eq!(
            r.wire(&wire),
            Bytes::from_static(
                b"POST /login HTTP/1.1\r\nHost: a.test\r\nAuthorization: Basic <redacted>\r\n\r\n{\"password\":\"<redacted>\"}"
            )
        );
    }

    #[test]
    fn binary_bodies_survive_redaction() {
        let r = redactor(&[]);
        let mut raw = b"HTTP/1.1 200 OK\r\nSet-Cookie: a=1\r\n\r\n".to_vec();
        raw.extend_from_slice(&[0x1f, 0x8b, 0xff, 0x00]);
        let out = r.wire(&Bytes::from(raw));
        assert!(out.starts_with(b"HTTP/1.1 200 OK\r\nSet-Cookie: a=<redacted>\r\n\r\n"));
        assert!(out.ends_with(&[0x1f, 0x8b, 0xff, 0x00]));
    }
}
