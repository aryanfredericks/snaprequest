//! AWS Signature Version 4 (header signing).
//!
//! Follows the AWS documentation and matches botocore's signer: every header that is sent
//! is signed except a few that proxies or clients change; the canonical URI is encoded
//! again for every service except S3; the query string is sent and signed in strict
//! RFC 3986 encoding.

use std::fmt::Write as _;
use std::time::SystemTime;

use ring::hmac;
use url::Url;

/// Headers never signed (they may change on the way, or carry the signature).
const UNSIGNED: [&str; 11] = [
    "authorization",
    "user-agent",
    "expect",
    "x-amzn-trace-id",
    "connection",
    "keep-alive",
    "proxy-connection",
    "proxy-authorization",
    "te",
    "transfer-encoding",
    "upgrade",
];

/// Characters left alone by AWS URI encoding (RFC 3986 unreserved).
const AWS_ENCODE: &percent_encoding::AsciiSet = &percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// Same, keeping `/` (for paths).
const AWS_ENCODE_PATH: &percent_encoding::AsciiSet = &AWS_ENCODE.remove(b'/');

pub(crate) const EMPTY_SHA256: &str =
    "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

pub(crate) fn should_sign(name: &str) -> bool {
    !UNSIGNED.contains(&name.to_ascii_lowercase().as_str())
}

/// `20150830T123600Z`.
pub(crate) fn amz_date(now: SystemTime) -> String {
    let t = time::OffsetDateTime::from(now);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        t.year(),
        u8::from(t.month()),
        t.day(),
        t.hour(),
        t.minute(),
        t.second()
    )
}

/// Rewrites the query of `url` in strict RFC 3986 encoding (`%20`, not `+`), so what is
/// sent is exactly what is signed.
pub(crate) fn strict_query(url: &mut Url) {
    let Some(q) = url.query() else {
        return;
    };
    if q.is_empty() {
        url.set_query(None);
        return;
    }
    let encoded = q
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            format!("{}={}", reencode(k), reencode(v))
        })
        .collect::<Vec<_>>()
        .join("&");
    url.set_query(Some(&encoded));
}

/// Decodes a query component (`+` is a space) and encodes it the AWS way.
fn reencode(s: &str) -> String {
    let spaced = s.replace('+', " ");
    let raw = percent_encoding::percent_decode_str(&spaced).collect::<Vec<u8>>();
    percent_encoding::percent_encode(&raw, AWS_ENCODE).to_string()
}

/// What gets signed.
pub(crate) struct Input<'a> {
    pub method: &'a str,
    /// Path as sent (already percent-encoded).
    pub path: &'a str,
    /// Query as sent, in strict encoding (see [`strict_query`]).
    pub query: Option<&'a str>,
    /// Headers as sent, in order; unsigned ones are skipped here.
    pub headers: &'a [(String, String)],
    /// Hex SHA-256 of the body (or `UNSIGNED-PAYLOAD`).
    pub payload_hash: &'a str,
    /// `20150830T123600Z`.
    pub timestamp: &'a str,
}

pub(crate) struct Credentials<'a> {
    pub access_key: &'a str,
    pub secret_key: &'a str,
    pub region: &'a str,
    pub service: &'a str,
}

/// The canonical request (exposed for tests and debugging).
pub(crate) fn canonical_request(i: &Input<'_>, service: &str) -> (String, String) {
    let path = if i.path.is_empty() { "/" } else { i.path };
    let canonical_uri = if service == "s3" {
        path.to_owned()
    } else {
        percent_encoding::utf8_percent_encode(path, AWS_ENCODE_PATH).to_string()
    };

    let mut pairs: Vec<(&str, &str)> = i
        .query
        .unwrap_or_default()
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| p.split_once('=').unwrap_or((p, "")))
        .collect();
    pairs.sort_unstable();
    let canonical_query = pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join("&");

    // Lowercase names, sorted; repeated headers joined with commas; whitespace collapsed.
    let mut names: Vec<String> = i
        .headers
        .iter()
        .map(|(n, _)| n.trim().to_ascii_lowercase())
        .filter(|n| should_sign(n))
        .collect();
    names.sort_unstable();
    names.dedup();
    let mut canonical_headers = String::new();
    for name in &names {
        let values: Vec<String> = i
            .headers
            .iter()
            .filter(|(n, _)| n.trim().eq_ignore_ascii_case(name))
            .map(|(_, v)| v.split_whitespace().collect::<Vec<_>>().join(" "))
            .collect();
        let _ = writeln!(canonical_headers, "{name}:{}", values.join(","));
    }
    let signed = names.join(";");

    (
        format!(
            "{}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed}\n{}",
            i.method.to_ascii_uppercase(),
            i.payload_hash
        ),
        signed,
    )
}

/// The `Authorization` header value.
pub(crate) fn authorization(i: &Input<'_>, c: &Credentials<'_>) -> String {
    let (canonical, signed_headers) = canonical_request(i, c.service);
    let date = &i.timestamp[..8.min(i.timestamp.len())];
    let scope = format!("{date}/{}/{}/aws4_request", c.region, c.service);
    let string_to_sign = format!(
        "AWS4-HMAC-SHA256\n{}\n{scope}\n{}",
        i.timestamp,
        sha256_hex(canonical.as_bytes())
    );
    let key = [date, c.region, c.service, "aws4_request"].iter().fold(
        format!("AWS4{}", c.secret_key).into_bytes(),
        |key, part| {
            hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &key), part.as_bytes())
                .as_ref()
                .to_vec()
        },
    );
    let signature = hmac::sign(
        &hmac::Key::new(hmac::HMAC_SHA256, &key),
        string_to_sign.as_bytes(),
    );
    format!(
        "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={}",
        c.access_key,
        hex(signature.as_ref())
    )
}

pub(crate) fn sha256_hex(data: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, data).as_ref())
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    const CREDS: Credentials<'static> = Credentials {
        access_key: "AKIDEXAMPLE",
        secret_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
        region: "us-east-1",
        service: "service",
    };

    fn headers(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(n, v)| ((*n).to_owned(), (*v).to_owned()))
            .collect()
    }

    /// Expected values computed with botocore 1.43 (`SigV4Auth` / `S3SigV4Auth`) at
    /// 2015-08-30T12:36:00Z; `get-vanilla` is also the AWS test suite's.
    #[test]
    fn matches_botocore() {
        let ts = "20150830T123600Z";
        let vanilla = authorization(
            &Input {
                method: "GET",
                path: "/",
                query: None,
                headers: &headers(&[("Host", "example.amazonaws.com"), ("X-Amz-Date", ts)]),
                payload_hash: EMPTY_SHA256,
                timestamp: ts,
            },
            &CREDS,
        );
        assert_eq!(
            vanilla,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );

        let body_hash = sha256_hex(b"Param1=value1");
        assert_eq!(
            body_hash,
            "9095672bbd1f56dfc5b65f3e153adc8731a4a654192329106275f4c7b24d0b6e"
        );
        let post = authorization(
            &Input {
                method: "POST",
                path: "/a%20b/c",
                query: Some("b=x%20y&a=1&a=0"),
                headers: &headers(&[
                    ("Host", "example.amazonaws.com"),
                    ("Content-Type", "application/x-www-form-urlencoded"),
                    ("My-Header1", "  a   b   c "),
                    ("my-header1", "d"),
                    ("User-Agent", "snapreq/test"),
                    ("X-Amz-Date", ts),
                ]),
                payload_hash: &body_hash,
                timestamp: ts,
            },
            &CREDS,
        );
        assert!(
            post.ends_with("SignedHeaders=content-type;host;my-header1;x-amz-date, Signature=3ac1c67aec586edaeff3ecaefbf7c4af368e995a4ccd84e04257d947f74a5f70"),
            "{post}"
        );

        let s3_hash = sha256_hex(b"hello");
        let s3 = authorization(
            &Input {
                method: "PUT",
                path: "/my%20file.txt",
                query: None,
                headers: &headers(&[
                    ("Host", "bucket.s3.amazonaws.com"),
                    ("Content-Type", "text/plain"),
                    ("X-Amz-Content-SHA256", &s3_hash),
                    ("X-Amz-Date", ts),
                    ("X-Amz-Security-Token", "SESSIONTOKEN"),
                ]),
                payload_hash: &s3_hash,
                timestamp: ts,
            },
            &Credentials {
                region: "eu-west-1",
                service: "s3",
                ..CREDS
            },
        );
        assert_eq!(
            s3,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/eu-west-1/s3/aws4_request, SignedHeaders=content-type;host;x-amz-content-sha256;x-amz-date;x-amz-security-token, Signature=96ff7e75f79bb4bdda1d005488024119748992cb4db8f9988ef53b437b558244"
        );
    }

    #[test]
    fn amz_dates() {
        let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_440_938_160);
        assert_eq!(amz_date(t), "20150830T123600Z");
    }

    #[test]
    fn queries_are_strictly_encoded() {
        let mut u = Url::parse("https://a.test/p?b=x+y&a=%7e&c&d=a%2Fb").unwrap();
        strict_query(&mut u);
        assert_eq!(u.query(), Some("b=x%20y&a=~&c=&d=a%2Fb"));
        let mut empty = Url::parse("https://a.test/p?").unwrap();
        strict_query(&mut empty);
        assert_eq!(empty.as_str(), "https://a.test/p");
    }
}
