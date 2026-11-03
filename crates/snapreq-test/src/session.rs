//! Running collection requests one after another, passing captured values along.

use snapreq_collection::{
    Collection, Environment, Scopes, Secrets, Variables, build_assertions, build_request,
};
use snapreq_core::{Client, Request, Response};

use crate::assert::{Assertions, Outcome};
use crate::capture::{Captured, Capturer};

/// The result of one request in a session.
#[derive(Debug)]
pub struct RequestRun {
    pub key: String,
    /// `None` if the request could not be built.
    pub request: Option<Request>,
    pub outcome: RunOutcome,
    pub assertions: Vec<Outcome>,
    pub captures: Vec<Captured>,
}

#[derive(Debug)]
pub enum RunOutcome {
    Response(Box<Response>),
    /// Building the request, its checks or captures failed; nothing was sent.
    Invalid(String),
    /// Sending failed.
    Failed(snapreq_core::Error),
}

impl RequestRun {
    pub fn response(&self) -> Option<&Response> {
        match &self.outcome {
            RunOutcome::Response(r) => Some(r),
            _ => None,
        }
    }

    /// A response arrived, every check passed and every capture found its value.
    pub fn passed(&self) -> bool {
        self.response().is_some()
            && self.assertions.iter().all(|a| a.passed)
            && self.captures.iter().all(|c| c.value.is_some())
    }
}

/// State shared by the requests of one run: runtime variables (`--var`s, then captured
/// values) and the secret values to hide.
#[derive(Debug)]
pub struct Session<'a> {
    collection: &'a Collection,
    environment: Option<&'a Environment>,
    secrets: &'a Secrets,
    client: Client,
    pub globals: Variables,
    /// Highest-precedence variables: set by the caller, then by captures.
    pub runtime: Variables,
    /// Secret values seen in this run, hidden in every later request.
    hidden: Vec<String>,
}

impl<'a> Session<'a> {
    pub fn new(
        collection: &'a Collection,
        environment: Option<&'a Environment>,
        secrets: &'a Secrets,
        client: Client,
    ) -> Self {
        Self {
            collection,
            environment,
            secrets,
            client,
            globals: Variables::default(),
            runtime: Variables::default(),
            hidden: Vec::new(),
        }
    }

    /// Hides `value` in every later request (for example a `--var` of a secret variable).
    pub fn hide(&mut self, value: impl Into<String>) {
        let v = value.into();
        if !v.is_empty() && !self.hidden.contains(&v) {
            self.hidden.push(v);
        }
    }

    /// Variables as a request at `key` would see them.
    pub fn scopes(&self, key: &str) -> Scopes {
        let mut scopes = Scopes::for_request(
            self.collection,
            key,
            self.environment,
            &self.globals,
            &self.runtime,
        );
        self.secrets.fill(&mut scopes);
        scopes
    }

    /// Builds and sends the request at `key`; `adjust` can change it before sending.
    /// Assertions are evaluated and captured values become runtime variables.
    pub async fn send(&mut self, key: &str, adjust: impl FnOnce(&mut Request)) -> RequestRun {
        let invalid = |request: Option<Request>, why: String| RequestRun {
            key: key.to_owned(),
            request,
            outcome: RunOutcome::Invalid(why),
            assertions: Vec::new(),
            captures: Vec::new(),
        };
        let scopes = self.scopes(key);
        let mut request = match build_request(self.collection, key, &scopes) {
            Ok(r) => r,
            Err(e) => return invalid(None, e.to_string()),
        };
        for v in &self.hidden {
            if !request.settings.secrets.contains(v) {
                request.settings.secrets.push(v.clone());
            }
        }
        adjust(&mut request);
        let checks = match build_assertions(self.collection, key, &scopes)
            .map_err(|e| e.to_string())
            .and_then(|spec| Assertions::compile(&spec))
        {
            Ok(a) => a,
            Err(e) => return invalid(Some(request), e),
        };
        let capturer = match self
            .collection
            .find(key)
            .map(|item| Capturer::compile(&item.file.capture))
            .transpose()
        {
            Ok(c) => c,
            Err(e) => return invalid(Some(request), e),
        };

        let outcome = match self.client.execute(&request).await {
            Ok(resp) => resp,
            Err(e) => {
                return RequestRun {
                    key: key.to_owned(),
                    request: Some(request),
                    outcome: RunOutcome::Failed(e),
                    assertions: Vec::new(),
                    captures: Vec::new(),
                };
            }
        };
        let assertions = checks.evaluate(&outcome);
        let captures = capturer.map(|c| c.extract(&outcome)).unwrap_or_default();
        for c in &captures {
            if let Some(value) = &c.value {
                if c.secret {
                    self.runtime.set_secret(&c.name, value);
                    self.hide(value.clone());
                } else {
                    self.runtime.set(&c.name, value);
                }
            }
        }
        RequestRun {
            key: key.to_owned(),
            request: Some(request),
            outcome: RunOutcome::Response(Box::new(outcome)),
            assertions,
            captures,
        }
    }
}
