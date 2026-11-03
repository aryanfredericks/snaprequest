//! Authentication helpers: applying a request's [`Auth`] to the hop being sent, Digest
//! and OAuth 2.0. Front ends plug in a [`TokenStore`] and an [`AuthorizationPrompt`]
//! through [`ClientConfig`](crate::ClientConfig).

use crate::model::Response;

use url::Url;

pub(crate) mod digest;
pub(crate) mod oauth2;
pub(crate) mod sigv4;

pub use oauth2::{AuthorizationPrompt, MemoryTokenStore, Token, TokenStore};

use crate::model::{ApiKeyLocation, Auth, Request, auth::basic_credentials};

/// The header `auth` adds, if any.
pub(crate) fn header(auth: &Auth) -> Option<(&str, String)> {
    match auth {
        Auth::None => None,
        Auth::Basic { username, password } => Some((
            "Authorization",
            format!("Basic {}", basic_credentials(username, password)),
        )),
        Auth::Bearer { token } => Some(("Authorization", format!("Bearer {}", token.trim()))),
        Auth::ApiKey {
            name,
            value,
            location: ApiKeyLocation::Header,
        } => Some((name.trim(), value.clone())),
        Auth::ApiKey { .. } | Auth::Digest { .. } | Auth::OAuth2(_) | Auth::AwsSigV4 { .. } => None,
    }
}

/// The header `auth` sets (now or after a challenge), if any.
fn header_name(auth: &Auth) -> Option<&str> {
    match auth {
        Auth::Digest { .. } | Auth::OAuth2(_) | Auth::AwsSigV4 { .. } => Some("Authorization"),
        other => header(other).map(|(name, _)| name),
    }
}

/// For Digest auth: the `Authorization` value answering the challenge in `resp`, if `resp`
/// is a `401` with one. `Err` explains a challenge that cannot be answered.
pub(crate) fn digest_retry(
    req: &Request,
    url: &Url,
    resp: &Response,
) -> Option<Result<String, String>> {
    let Auth::Digest { username, password } = &req.auth else {
        return None;
    };
    if resp.status != 401 || shadowed_by_header(req).is_some() {
        return None;
    }
    let challenges = digest::parse_challenges(
        resp.headers
            .iter()
            .filter(|h| h.name.eq_ignore_ascii_case("www-authenticate"))
            .map(|h| h.value.as_str()),
    );
    let uri = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    };
    let cnonce = digest::cnonce();
    let creds = digest::Credentials {
        username,
        password,
        method: req.method.as_str(),
        uri: &uri,
        cnonce: &cnonce,
    };
    match digest::authorization(&challenges, &creds) {
        Ok(value) => Some(Ok(value)),
        Err(digest::DigestError::NoChallenge) => None,
        Err(digest::DigestError::Unsupported(why)) => Some(Err(why)),
    }
}

/// Adds a query API key to `url`, replacing parameters of the same name so that applying
/// it again (to a redirect target that echoes the key) does not repeat it.
pub(crate) fn apply_to_url(auth: &Auth, url: &mut Url) {
    if let Auth::AwsSigV4 { .. } = auth {
        // Sent exactly as signed.
        sigv4::strict_query(url);
        return;
    }
    let Auth::ApiKey {
        name,
        value,
        location: ApiKeyLocation::Query,
    } = auth
    else {
        return;
    };
    let kept: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(n, _)| n != name)
        .map(|(n, v)| (n.into_owned(), v.into_owned()))
        .collect();
    url.set_query(None);
    let mut pairs = url.query_pairs_mut();
    pairs.extend_pairs(kept);
    pairs.append_pair(name, value);
}

/// Whether the request's own enabled headers already set the header `auth` would add.
/// Hand-written headers win, like `-H` over `-u` in curl.
pub(crate) fn shadowed_by_header(req: &Request) -> Option<String> {
    let name = header_name(&req.auth)?;
    req.enabled_headers()
        .find(|h| h.name.trim().eq_ignore_ascii_case(name))
        .map(|h| h.name.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headers() {
        let basic = Auth::Basic {
            username: "Aladdin".into(),
            password: "open sesame".into(),
        };
        assert_eq!(
            header(&basic).unwrap(),
            ("Authorization", "Basic QWxhZGRpbjpvcGVuIHNlc2FtZQ==".into())
        );
        let utf8 = Auth::Basic {
            username: "test".into(),
            password: "123£".into(),
        };
        // RFC 7617 section 2.1 example.
        assert_eq!(header(&utf8).unwrap().1, "Basic dGVzdDoxMjPCow==");
        assert_eq!(
            header(&Auth::Bearer {
                token: "t0k".into()
            })
            .unwrap(),
            ("Authorization", "Bearer t0k".into())
        );
        let key = Auth::ApiKey {
            name: " X-API-Key ".into(),
            value: "k".into(),
            location: ApiKeyLocation::Header,
        };
        assert_eq!(header(&key).unwrap(), ("X-API-Key", "k".into()));
        assert!(header(&Auth::None).is_none());
    }

    #[test]
    fn query_key_is_added_once() {
        let auth = Auth::ApiKey {
            name: "api_key".into(),
            value: "a b&c".into(),
            location: ApiKeyLocation::Query,
        };
        assert!(header(&auth).is_none());
        let mut url = Url::parse("https://api.test/x?q=1&api_key=old").unwrap();
        apply_to_url(&auth, &mut url);
        apply_to_url(&auth, &mut url);
        assert_eq!(url.as_str(), "https://api.test/x?q=1&api_key=a+b%26c");

        let mut plain = Url::parse("https://api.test/x").unwrap();
        apply_to_url(&Auth::Bearer { token: "t".into() }, &mut plain);
        assert_eq!(plain.as_str(), "https://api.test/x");
    }

    #[test]
    fn explicit_headers_win() {
        let mut req = Request::get("http://a.test").header("authorization", "Custom x");
        req.auth = Auth::Bearer { token: "t".into() };
        assert_eq!(shadowed_by_header(&req).as_deref(), Some("authorization"));
        req.headers[0].enabled = false;
        assert!(shadowed_by_header(&req).is_none());
    }
}
