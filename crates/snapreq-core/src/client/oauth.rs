//! Getting OAuth 2.0 access tokens for a request: cache, refresh, grants.

use std::future::Future;
use std::pin::Pin;
use std::time::SystemTime;

use super::Client;
use super::recorder::Recorder;
use crate::auth::Token;
use crate::auth::oauth2::{self as o2, AUTHORIZATION_TIMEOUT};
use crate::model::{OAuth2, OAuth2Grant, Phase, Request, Response, TraceEvent};
use crate::{Error, Result};

/// The token used for one execution.
#[derive(Debug, Clone)]
pub(super) struct OAuthToken {
    access_token: String,
    /// Whether it came from the cache (and so may be stale even if not expired).
    pub from_cache: bool,
}

impl OAuthToken {
    pub(super) fn header(&self) -> String {
        format!("Bearer {}", self.access_token)
    }
}

type BoxedResponse<'a> = Pin<Box<dyn Future<Output = Result<Response>> + Send + 'a>>;

impl Client {
    /// Returns a usable token for `o`: from the cache unless `force_new`, else refreshed,
    /// else from a new grant. Problems become a `Phase::Auth` failure with the trace.
    pub(super) async fn oauth_token(
        &self,
        req: &Request,
        o: &OAuth2,
        rec: &mut Recorder,
        force_new: bool,
    ) -> Result<OAuthToken> {
        let previous = rec.phase;
        rec.phase = Phase::Auth;
        let result = self.oauth_token_inner(req, o, rec, force_new).await;
        rec.phase = previous;
        let token = result?;
        rec.add_secret(&token.access_token);
        Ok(token)
    }

    async fn oauth_token_inner(
        &self,
        req: &Request,
        o: &OAuth2,
        rec: &mut Recorder,
        force_new: bool,
    ) -> Result<OAuthToken> {
        let store = self.inner.config.token_store.clone();
        let key = o2::cache_key(o);
        let now = SystemTime::now();
        let cached = store.get(&key);

        if let Some(t) = &cached {
            if !force_new && t.is_fresh(now) {
                rec.record(TraceEvent::Info {
                    message: match t.expires_in(now) {
                        Some(s) => {
                            format!("oauth2: using the cached access token (expires in {s} s)")
                        }
                        None => "oauth2: using the cached access token".into(),
                    },
                });
                return Ok(OAuthToken {
                    access_token: t.access_token.clone(),
                    from_cache: true,
                });
            }
            if let Some(refresh) = &t.refresh_token {
                rec.add_secret(refresh);
                let call = o2::token_request(o, o2::refresh_params(o, refresh), false, req);
                match self.token_call(&call, rec).await? {
                    Ok(mut fresh) => {
                        // Servers may keep the refresh token and not send it again.
                        if fresh.refresh_token.is_none() {
                            fresh.refresh_token = Some(refresh.clone());
                        }
                        store.put(&key, &fresh);
                        rec.record(TraceEvent::Info {
                            message: format!(
                                "oauth2: refreshed the access token at {}",
                                o.token_url.trim()
                            ),
                        });
                        return Ok(OAuthToken {
                            access_token: fresh.access_token,
                            from_cache: false,
                        });
                    }
                    Err(why) => rec.record(TraceEvent::Info {
                        message: format!(
                            "oauth2: refreshing failed ({why}); requesting a new token"
                        ),
                    }),
                }
            }
            store.remove(&key);
        }

        let call = match o.grant {
            OAuth2Grant::ClientCredentials | OAuth2Grant::Password => {
                o2::token_request(o, o2::grant_params(o), true, req)
            }
            OAuth2Grant::AuthorizationCode => self.authorize(req, o, rec).await?,
        };
        match self.token_call(&call, rec).await? {
            Ok(token) => {
                store.put(&key, &token);
                rec.record(TraceEvent::Info {
                    message: format!(
                        "oauth2: got an access token from {} ({} grant{})",
                        o.token_url.trim(),
                        o.grant.as_str(),
                        token
                            .expires_in(SystemTime::now())
                            .map(|s| format!(", expires in {s} s"))
                            .unwrap_or_default()
                    ),
                });
                Ok(OAuthToken {
                    access_token: token.access_token,
                    from_cache: false,
                })
            }
            Err(why) => Err(rec.fail(why, false)),
        }
    }

    /// Sends a token request. The outer `Result` fails the execution (network problems);
    /// the inner one is the token endpoint's verdict.
    async fn token_call(
        &self,
        call: &Request,
        rec: &mut Recorder,
    ) -> Result<std::result::Result<Token, String>> {
        // Boxed to keep the `execute` future small.
        let fut: BoxedResponse<'_> = Box::pin(self.execute_plain(call));
        let resp = match fut.await {
            Ok(resp) => resp,
            Err(Error::Execution(e)) => {
                return Err(rec.fail(
                    format!("token request to {} failed: {}", call.url, e),
                    e.timed_out,
                ));
            }
            Err(e) => return Err(rec.fail(format!("token request to {}: {e}", call.url), false)),
        };
        let parsed = o2::parse_token_response(&resp, SystemTime::now());
        if let Ok(t) = &parsed {
            rec.add_secret(&t.access_token);
            if let Some(r) = &t.refresh_token {
                rec.add_secret(r);
            }
        }
        Ok(parsed)
    }

    /// The browser part of the authorization code grant; returns the code exchange request.
    async fn authorize(&self, req: &Request, o: &OAuth2, rec: &mut Recorder) -> Result<Request> {
        let Some(prompt) = self.inner.config.authorization_prompt.clone() else {
            return Err(rec.fail(
                "the authorization code grant needs a browser; this front end cannot show one",
                false,
            ));
        };
        let (listener, redirect_uri) = o2::redirect_listener(o)
            .await
            .map_err(|e| rec.fail(e, false))?;
        let (verifier, challenge) = o2::pkce();
        let state = o2::random_state();
        let url = o2::authorization_url(o, &redirect_uri, &state, &challenge)
            .map_err(|e| rec.fail(e, false))?;
        rec.record(TraceEvent::Info {
            message: format!(
                "oauth2: waiting for authorization in the browser (redirect to {redirect_uri})"
            ),
        });
        prompt
            .open(url.as_str())
            .map_err(|e| rec.fail(format!("cannot show the authorization page: {e}"), false))?;
        let code = match tokio::time::timeout(
            AUTHORIZATION_TIMEOUT,
            o2::wait_for_code(&listener, &redirect_uri, &state),
        )
        .await
        {
            Ok(Ok(code)) => code,
            Ok(Err(e)) => return Err(rec.fail(e, false)),
            Err(_) => {
                return Err(rec.fail(
                    format!(
                        "no authorization within {} s",
                        AUTHORIZATION_TIMEOUT.as_secs()
                    ),
                    true,
                ));
            }
        };
        rec.add_secret(&code);
        rec.add_secret(&verifier);
        let params = vec![
            ("grant_type", "authorization_code".to_owned()),
            ("code", code),
            ("redirect_uri", redirect_uri.to_string()),
            ("code_verifier", verifier),
        ];
        Ok(o2::token_request(o, params, false, req))
    }
}
