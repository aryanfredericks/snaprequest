//! The client's cookie jar (RFC 6265 rules via `cookie_store`).

use std::sync::{Mutex, MutexGuard};

use cookie_store::{CookieDomain, CookieExpiration, CookieStore, RawCookie, StoreAction};
use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use url::Url;

/// A cookie held in the jar, for display and export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    pub domain: String,
    /// `true` if the cookie is only sent to exactly `domain` (no `Domain` attribute).
    pub host_only: bool,
    pub path: String,
    /// Expiry (RFC 3339); `None` for session cookies.
    pub expires: Option<String>,
    pub secure: bool,
    pub http_only: bool,
}

#[derive(Debug, Default)]
pub(crate) struct Jar(Mutex<CookieStore>);

impl Jar {
    fn lock(&self) -> MutexGuard<'_, CookieStore> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Cookies to send to `url`, as `(name, value)` pairs.
    pub(crate) fn for_request(&self, url: &Url) -> Vec<(String, String)> {
        self.lock()
            .get_request_values(url)
            .map(|(n, v)| (n.to_owned(), v.to_owned()))
            .collect()
    }

    /// Stores the cookies from `Set-Cookie` header values received from `url`.
    /// Returns a note per cookie for the trace.
    pub(crate) fn store<'a>(
        &self,
        url: &Url,
        set_cookie: impl Iterator<Item = &'a str>,
    ) -> Vec<String> {
        let mut store = self.lock();
        let mut notes = Vec::new();
        for header in set_cookie {
            let raw = match RawCookie::parse(header.to_owned()) {
                Ok(raw) => raw,
                Err(e) => {
                    notes.push(format!("ignored malformed Set-Cookie `{header}`: {e}"));
                    continue;
                }
            };
            let name = raw.name().to_owned();
            notes.push(match store.insert_raw(&raw, url) {
                Ok(StoreAction::Inserted) => format!("stored cookie `{name}`"),
                Ok(StoreAction::UpdatedExisting) => format!("updated cookie `{name}`"),
                Ok(StoreAction::ExpiredExisting) => format!("server deleted cookie `{name}`"),
                Err(e) => format!("rejected cookie `{name}`: {e}"),
            });
        }
        notes
    }

    /// Every unexpired cookie in the jar.
    pub(crate) fn list(&self) -> Vec<StoredCookie> {
        self.lock()
            .iter_unexpired()
            .map(|c| {
                let (domain, host_only) = match &c.domain {
                    CookieDomain::HostOnly(d) => (d.clone(), true),
                    CookieDomain::Suffix(d) => (d.clone(), false),
                    CookieDomain::NotPresent | CookieDomain::Empty => (String::new(), true),
                };
                StoredCookie {
                    name: c.name().to_owned(),
                    value: c.value().to_owned(),
                    domain,
                    host_only,
                    path: String::from(&c.path),
                    expires: match &c.expires {
                        CookieExpiration::AtUtc(t) => t.format(&Rfc3339).ok(),
                        CookieExpiration::SessionEnd => None,
                    },
                    secure: c.secure().unwrap_or(false),
                    http_only: c.http_only().unwrap_or(false),
                }
            })
            .collect()
    }

    pub(crate) fn clear(&self) {
        self.lock().clear();
    }
}

/// Merges jar cookies into a user-supplied `Cookie` header value. Cookies the user
/// set explicitly win over jar cookies with the same name.
pub(crate) fn merge(user: Option<&str>, jar: &[(String, String)]) -> String {
    let user = user.map(str::trim).filter(|u| !u.is_empty());
    let user_names: Vec<&str> = user
        .into_iter()
        .flat_map(|u| u.split(';'))
        .filter_map(|pair| pair.split_once('=').map(|(n, _)| n.trim()))
        .collect();
    let from_jar = jar
        .iter()
        .filter(|(n, _)| !user_names.contains(&n.as_str()))
        .map(|(n, v)| format!("{n}={v}"));
    user.map(str::to_owned)
        .into_iter()
        .chain(from_jar)
        .collect::<Vec<_>>()
        .join("; ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn stores_and_returns_matching_cookies() {
        let jar = Jar::default();
        let notes = jar.store(
            &url("https://api.test/login"),
            [
                "sid=abc; Path=/; HttpOnly",
                "pref=dark; Domain=api.test; Path=/settings",
                "secure_only=1; Secure",
            ]
            .into_iter(),
        );
        assert_eq!(
            notes,
            [
                "stored cookie `sid`",
                "stored cookie `pref`",
                "stored cookie `secure_only`"
            ]
        );

        let names = |u: &str| {
            let mut n: Vec<String> = jar
                .for_request(&url(u))
                .into_iter()
                .map(|(n, _)| n)
                .collect();
            n.sort();
            n
        };
        assert_eq!(names("https://api.test/"), ["secure_only", "sid"]);
        assert_eq!(
            names("https://api.test/settings/x"),
            ["pref", "secure_only", "sid"]
        );
        assert_eq!(names("https://sub.api.test/settings"), ["pref"]);
        // Secure cookies are not sent over plain HTTP.
        assert_eq!(names("http://api.test/"), ["sid"]);
        assert!(names("https://other.test/").is_empty());
    }

    #[test]
    fn deletion_rejection_and_listing() {
        let jar = Jar::default();
        jar.store(&url("https://a.test/"), ["a=1; Max-Age=3600"].into_iter());
        let notes = jar.store(
            &url("https://a.test/"),
            ["a=; Max-Age=0", "x=1; Domain=other.test", "=nameless"].into_iter(),
        );
        assert_eq!(notes[0], "server deleted cookie `a`");
        assert!(notes[1].starts_with("rejected cookie `x`"), "{}", notes[1]);
        assert_eq!(notes.len(), 3);
        assert!(jar.list().is_empty());

        jar.store(
            &url("https://a.test/p/q"),
            ["b=2; Max-Age=60; Secure"].into_iter(),
        );
        let listed = jar.list();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].name, "b");
        assert_eq!(listed[0].domain, "a.test");
        assert!(listed[0].host_only && listed[0].secure);
        assert_eq!(listed[0].path, "/p");
        assert!(listed[0].expires.is_some());

        jar.clear();
        assert!(jar.list().is_empty());
    }

    #[test]
    fn merging_with_user_cookies() {
        let jar = vec![
            ("a".to_owned(), "jar".to_owned()),
            ("b".to_owned(), "2".to_owned()),
        ];
        assert_eq!(merge(None, &jar), "a=jar; b=2");
        assert_eq!(merge(Some("a=mine"), &jar), "a=mine; b=2");
        assert_eq!(merge(Some("c=3"), &[]), "c=3");
    }
}
