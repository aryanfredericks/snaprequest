//! Applying a request's [`Auth`] to the hop being sent.

use url::Url;

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
        Auth::ApiKey { .. } => None,
    }
}

/// Adds a query API key to `url`, replacing parameters of the same name so that applying
/// it again (to a redirect target that echoes the key) does not repeat it.
pub(crate) fn apply_to_url(auth: &Auth, url: &mut Url) {
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
    let (name, _) = header(&req.auth)?;
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
