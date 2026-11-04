//! Helpers shared by the importers.

use base64::Engine as _;
use snapreq_collection::{AuthSpec, BodySpec, KeyValues};
use snapreq_core::model::KeyValue;

use crate::model::Imported;

/// A file-name-friendly key: lower-case ASCII letters, digits and `-`, at most 60 characters.
pub(crate) fn slug(name: &str, fallback: &str) -> String {
    let mut out = String::new();
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let mut out: String = out.trim_matches('-').chars().take(60).collect();
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        fallback.to_owned()
    } else {
        out
    }
}

/// A valid variable name made from `hint` (letters, digits, `_`, `-`, `.`).
pub(crate) fn var_name(hint: &str) -> String {
    let mut out: String = hint
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') {
                c
            } else {
                '_'
            }
        })
        .collect();
    if out.is_empty() {
        out = "value".into();
    }
    out
}

/// Names of the `{{name}}` placeholders in `text` (escaped `\{{` ones excluded).
pub(crate) fn placeholders(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut prev_backslash = false;
    while let Some(i) = rest.find("{{") {
        let escaped = if i == 0 {
            prev_backslash
        } else {
            rest.as_bytes()[i - 1] == b'\\'
        };
        let after = &rest[i + 2..];
        if let Some(end) = after.find("}}") {
            let name = after[..end].trim();
            if !escaped
                && !name.is_empty()
                && name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '$'))
            {
                out.push(name.to_owned());
            }
            prev_backslash = end > 0 && after.as_bytes()[end - 1] == b'\\';
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    out
}

/// Escapes `{{` so recorded text is sent literally, not as a placeholder.
pub(crate) fn literal(text: &str) -> String {
    text.replace("{{", "\\{{")
}

/// The URL without its query and fragment, and its query parameters (percent-decoded).
pub(crate) fn split_query(url: &str) -> (String, Vec<KeyValue>) {
    let url = url.split('#').next().unwrap_or(url);
    let Some((base, query)) = url.split_once('?') else {
        return (url.to_owned(), Vec::new());
    };
    (base.to_owned(), parse_pairs(query))
}

/// `a=1&b=2` (percent-decoded; `+` is a space).
pub(crate) fn parse_pairs(text: &str) -> Vec<KeyValue> {
    text.split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
            KeyValue::new(decode(k), decode(v))
        })
        .collect()
}

fn decode(s: &str) -> String {
    let s = s.replace('+', " ");
    percent_encoding::percent_decode_str(&s)
        .decode_utf8_lossy()
        .into_owned()
}

/// Percent-encodes a form value (`application/x-www-form-urlencoded`).
pub(crate) fn form_encode(s: &str) -> String {
    url::form_urlencoded::byte_serialize(s.as_bytes()).collect()
}

/// The media type without parameters, lower-cased.
pub(crate) fn media_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

pub(crate) fn is_json_type(content_type: &str) -> bool {
    let m = media_type(content_type);
    m == "application/json" || m.ends_with("+json") || m == "text/json"
}

/// The value of the first enabled header named `name`.
pub(crate) fn header<'a>(headers: &'a KeyValues, name: &str) -> Option<&'a str> {
    headers
        .0
        .iter()
        .find(|h| h.enabled && h.name.eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str())
}

/// A body for `text` sent as `content_type`.
pub(crate) fn text_body(content_type: Option<&str>, text: &str) -> BodySpec {
    match content_type {
        Some(ct) if is_json_type(ct) => BodySpec::Json {
            content: Some(text.to_owned()),
            file: None,
        },
        Some(ct) if media_type(ct) == "application/x-www-form-urlencoded" => BodySpec::Form {
            fields: parse_pairs(text).into(),
        },
        Some(ct) => BodySpec::Text {
            content: Some(text.to_owned()),
            file: None,
            content_type: Some(ct.to_owned()),
        },
        None if looks_like_json(text) => BodySpec::Json {
            content: Some(text.to_owned()),
            file: None,
        },
        None => BodySpec::Text {
            content: Some(text.to_owned()),
            file: None,
            content_type: None,
        },
    }
}

fn looks_like_json(text: &str) -> bool {
    let t = text.trim_start();
    (t.starts_with('{') || t.starts_with('['))
        && serde_json::from_str::<serde_json::Value>(text).is_ok()
}

/// Drops headers the body already implies (a multipart boundary from the source would be wrong)
/// and headers the client computes.
pub(crate) fn tidy_headers(headers: &mut KeyValues, body: Option<&BodySpec>) {
    headers.0.retain(|h| {
        let name = h.name.to_ascii_lowercase();
        if matches!(
            name.as_str(),
            "content-length" | "host" | "connection" | "transfer-encoding" | "keep-alive" | "te"
        ) || name.starts_with(':')
        {
            return false;
        }
        if name != "content-type" {
            return true;
        }
        let m = media_type(&h.value);
        match body {
            Some(BodySpec::Json { .. }) => m != "application/json",
            Some(BodySpec::Form { .. }) => m != "application/x-www-form-urlencoded",
            Some(BodySpec::Multipart { .. }) => !m.starts_with("multipart/"),
            Some(BodySpec::Text { content_type, .. }) => content_type.is_none(),
            _ => true,
        }
    });
}

/// Header names whose values are credentials.
fn is_credential_header(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    matches!(
        n.as_str(),
        "x-api-key"
            | "api-key"
            | "apikey"
            | "x-apikey"
            | "x-auth-token"
            | "x-access-token"
            | "private-token"
            | "x-amz-security-token"
            | "proxy-authorization"
    ) || n.ends_with("-api-key")
        || n.ends_with("-token") && !n.contains("csrf") && !n.contains("xsrf")
}

/// Moves credentials out of literal headers: `Authorization: Bearer/Basic` becomes auth (when
/// `auth` is still unset), and every other credential (other `Authorization` schemes, `Cookie`,
/// API key headers) a secret variable.
pub(crate) fn protect_headers(
    imported: &mut Imported,
    headers: &mut KeyValues,
    auth: &mut Option<AuthSpec>,
) {
    let mut kept = Vec::with_capacity(headers.0.len());
    for mut h in std::mem::take(&mut headers.0) {
        let lower = h.name.to_ascii_lowercase();
        if lower == "authorization"
            && h.enabled
            && auth.is_none()
            && !h.value.contains("{{")
            && let Some(found) = auth_from_header(imported, &h.value)
        {
            *auth = Some(found);
            continue;
        }
        if lower == "authorization" || lower == "cookie" || is_credential_header(&lower) {
            h.value = imported.secret(&lower.replace('-', "_"), &h.value);
        }
        kept.push(h);
    }
    headers.0 = kept;
}

fn auth_from_header(imported: &mut Imported, value: &str) -> Option<AuthSpec> {
    let (scheme, rest) = value.trim().split_once(' ')?;
    let rest = rest.trim();
    if scheme.eq_ignore_ascii_case("bearer") && !rest.is_empty() {
        return Some(AuthSpec::Bearer {
            token: imported.secret("token", rest),
        });
    }
    if scheme.eq_ignore_ascii_case("basic") {
        let raw = base64::engine::general_purpose::STANDARD
            .decode(rest)
            .ok()?;
        let text = String::from_utf8(raw).ok()?;
        let (user, password) = text.split_once(':').unwrap_or((&text, ""));
        return Some(AuthSpec::Basic {
            username: user.to_owned(),
            password: imported.secret("password", password),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_and_names() {
        assert_eq!(slug("Create a User!", "x"), "create-a-user");
        assert_eq!(slug("  ", "request"), "request");
        assert_eq!(slug("GET /users/{id}", "x"), "get-users-id");
        assert_eq!(slug(&"a".repeat(80), "x").len(), 60);
        assert_eq!(var_name("api key"), "api_key");
    }

    #[test]
    fn placeholders_are_found_unless_escaped() {
        assert_eq!(
            placeholders("{{a}} and {{ b.c }} but \\{{d}} {{$uuid}} {{x y}}"),
            ["a", "b.c", "$uuid"]
        );
    }

    #[test]
    fn queries_split() {
        let (base, q) = split_query("https://a.test/p?x=1&y=a%20b&z=c+d&flag#frag");
        assert_eq!(base, "https://a.test/p");
        let pairs: Vec<_> = q.iter().map(|k| (&*k.name, &*k.value)).collect();
        assert_eq!(
            pairs,
            [("x", "1"), ("y", "a b"), ("z", "c d"), ("flag", "")]
        );
    }

    #[test]
    fn credential_headers_become_secrets() {
        let mut imp = Imported::new("t");
        let mut headers: KeyValues = vec![
            KeyValue::new("Authorization", "Bearer abc"),
            KeyValue::new("Cookie", "sid=1"),
            KeyValue::new("X-Api-Key", "k"),
            KeyValue::new("X-Csrf-Token", "c"),
        ]
        .into();
        let mut auth = None;
        protect_headers(&mut imp, &mut headers, &mut auth);
        assert_eq!(
            auth,
            Some(AuthSpec::Bearer {
                token: "{{token}}".into()
            })
        );
        let values: Vec<_> = headers.0.iter().map(|h| h.value.as_str()).collect();
        assert_eq!(values, ["{{cookie}}", "{{x_api_key}}", "c"]);
        assert_eq!(imp.collection.vars.0.len(), 3);
        assert!(imp.collection.vars.0.iter().all(|v| v.secret));
    }
}
