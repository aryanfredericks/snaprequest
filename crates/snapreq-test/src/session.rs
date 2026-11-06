//! Running collection requests one after another, passing captured values along.

use std::path::Path;

use snapreq_collection::{
    AssertSpec, Captures, Collection, Environment, RequestFile, Scopes, ScriptSpec, Secrets,
    Variables, build_file, build_request, interpolate_assertions,
};
use snapreq_core::{Client, Request, Response};
use snapreq_script::Phase;

use crate::assert::{Assertions, Outcome};
use crate::capture::{Captured, Capturer};
use crate::script::{self, Scripts};

/// The result of one request in a session.
#[derive(Debug)]
pub struct RequestRun {
    pub key: String,
    /// `None` if the request could not be built.
    pub request: Option<Request>,
    pub outcome: RunOutcome,
    pub assertions: Vec<Outcome>,
    /// Results of a flow step's `retry.until` checks (empty otherwise).
    pub until: Vec<Outcome>,
    pub captures: Vec<Captured>,
    /// `console.log` output of the request's scripts (secret values hidden).
    pub logs: Vec<String>,
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
            && self.until.iter().all(|a| a.passed)
            && self.captures.iter().all(|c| c.value.is_some())
    }

    fn invalid(key: &str, request: Option<Request>, why: String) -> Self {
        Self {
            key: key.to_owned(),
            request,
            outcome: RunOutcome::Invalid(why),
            assertions: Vec::new(),
            until: Vec::new(),
            captures: Vec::new(),
            logs: Vec::new(),
        }
    }

    fn with_logs(mut self, logs: Vec<String>) -> Self {
        self.logs = logs;
        self
    }
}

/// A progress event of a run (for live views).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Progress {
    /// A request is about to be sent.
    RequestStarted { iteration: usize, key: String },
    /// A request finished (or could not be built or sent).
    RequestDone {
        iteration: usize,
        key: String,
        passed: bool,
        status: Option<u16>,
        duration_ms: Option<f64>,
    },
}

/// Receives [`Progress`] events.
pub type ProgressSink = std::sync::Arc<dyn Fn(&Progress) + Send + Sync>;

/// Live observers of a run: trace events of every request and progress events.
#[derive(Clone, Default)]
pub struct Live {
    pub trace: Option<snapreq_core::TraceSink>,
    pub progress: Option<ProgressSink>,
}

impl std::fmt::Debug for Live {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Live")
            .field("trace", &self.trace.is_some())
            .field("progress", &self.progress.is_some())
            .finish()
    }
}

/// What to send.
#[derive(Debug, Clone, Copy)]
pub enum Target<'t> {
    /// A request of the collection, by key.
    Request(&'t str),
    /// A request that is not a collection item (an inline flow step), built as if it
    /// were the file `path` at `key` (for inheritance and relative paths).
    Inline {
        key: &'t str,
        file: &'t RequestFile,
        path: &'t Path,
    },
}

/// A WebSocket request ready to open ([`Session::websocket`]).
#[derive(Debug)]
pub struct WsPrepared {
    pub request: Request,
    pub options: snapreq_core::client::ws::WsOptions,
    /// Saved messages by name, placeholders resolved.
    pub messages: Vec<(String, snapreq_core::client::ws::WsMessage)>,
    /// Pre-request script output.
    pub logs: Vec<String>,
    /// Values to hide in what is shown (secrets of the request and of its messages).
    pub hidden: Vec<String>,
}

/// A request built by [`Session::build_for`].
struct Built {
    request: Request,
    scopes: Scopes,
    /// The request file as a pre-request script changed it.
    changed: Option<RequestFile>,
    logs: Vec<String>,
    test_codes: Vec<(String, String)>,
}

/// One send with everything a flow step can add; [`SendSpec::request`] is a plain
/// collection request.
#[derive(Debug, Clone, Copy)]
pub struct SendSpec<'t> {
    pub target: Target<'t>,
    /// Below captured values (a flow's `[vars]`).
    pub lower_vars: Option<&'t Variables>,
    /// Above captured values, below the session's overrides (a step's `vars`).
    pub upper_vars: Option<&'t Variables>,
    /// Extra checks and the file they come from (for schema paths and errors).
    pub assert: Option<(&'t AssertSpec, &'t Path)>,
    pub capture: Option<&'t Captures>,
    /// `retry.until` checks; evaluated into [`RequestRun::until`].
    pub until: Option<(&'t AssertSpec, &'t Path)>,
    /// A flow step's `[script]` and the file it is in.
    pub script: Option<(&'t ScriptSpec, &'t Path)>,
}

impl<'t> SendSpec<'t> {
    pub fn request(key: &'t str) -> Self {
        Self {
            target: Target::Request(key),
            lower_vars: None,
            upper_vars: None,
            assert: None,
            capture: None,
            until: None,
            script: None,
        }
    }
}

/// What a session has learned so far: captured values and the secret values to hide. A
/// snapshot taken before a flow step lets the flow be resumed at that step later.
#[derive(Debug, Clone, Default)]
pub struct SessionState {
    pub runtime: Variables,
    hidden: Vec<String>,
}

impl SessionState {
    /// Secret values hidden from requests sent with this state.
    pub fn hidden(&self) -> &[String] {
        &self.hidden
    }
}

/// State shared by the requests of one run.
///
/// Variable precedence, highest first: `overrides` (`--var`) → step variables → the data
/// row (`data`) → captured values (`runtime`) → flow variables → the request's usual scopes.
pub struct Session<'a> {
    collection: &'a Collection,
    environment: Option<&'a Environment>,
    secrets: &'a Secrets,
    client: Client,
    pub globals: Variables,
    /// Set by the caller (`--var`): win over everything.
    pub overrides: Variables,
    /// The current row of a data-driven run.
    pub data: Variables,
    /// Captured values.
    pub runtime: Variables,
    /// Secret values seen in this run, hidden in every later request.
    hidden: Vec<String>,
    /// Receives the trace events of every request as they happen.
    pub trace: Option<snapreq_core::TraceSink>,
    /// Receives progress events.
    pub progress: Option<ProgressSink>,
    /// Iteration number reported in progress events.
    pub iteration: usize,
}

impl std::fmt::Debug for Session<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("collection", &self.collection.root)
            .field("iteration", &self.iteration)
            .finish_non_exhaustive()
    }
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
            overrides: Variables::default(),
            data: Variables::default(),
            runtime: Variables::default(),
            hidden: Vec::new(),
            trace: None,
            progress: None,
            iteration: 1,
        }
    }

    pub fn collection(&self) -> &'a Collection {
        self.collection
    }

    /// Captured values and hidden secrets, for resuming later.
    pub fn state(&self) -> SessionState {
        SessionState {
            runtime: self.runtime.clone(),
            hidden: self.hidden.clone(),
        }
    }

    /// Continues from a saved state (its captured values; its secrets stay hidden).
    pub fn restore(&mut self, state: &SessionState) {
        self.runtime = state.runtime.clone();
        for v in &state.hidden {
            self.hide(v.clone());
        }
    }

    /// Hides `value` in every later request (for example a `--var` of a secret variable).
    pub fn hide(&mut self, value: impl Into<String>) {
        let v = value.into();
        if !v.is_empty() && !self.hidden.contains(&v) {
            self.hidden.push(v);
        }
    }

    /// The highest-precedence layer for one send.
    fn runtime_layer(&self, lower: Option<&Variables>, upper: Option<&Variables>) -> Variables {
        let mut out = Variables::default();
        for layer in [
            lower,
            Some(&self.runtime),
            Some(&self.data),
            upper,
            Some(&self.overrides),
        ]
        .into_iter()
        .flatten()
        {
            for v in &layer.0 {
                match out.0.iter_mut().find(|o| o.name == v.name) {
                    // A secret stays secret when a later layer sets it again.
                    Some(o) => {
                        o.value = v.value.clone().or(o.value.take());
                        o.secret |= v.secret;
                    }
                    None => out.0.push(v.clone()),
                }
            }
        }
        out
    }

    /// Variables as the request at `key` sees them, with extra layers.
    pub fn scopes_with(
        &self,
        key: &str,
        lower: Option<&Variables>,
        upper: Option<&Variables>,
    ) -> Scopes {
        let runtime = self.runtime_layer(lower, upper);
        let mut scopes = Scopes::for_request(
            self.collection,
            key,
            self.environment,
            &self.globals,
            &runtime,
        );
        self.secrets.fill(&mut scopes);
        scopes
    }

    /// Variables as the request at `key` would see them.
    pub fn scopes(&self, key: &str) -> Scopes {
        self.scopes_with(key, None, None)
    }

    /// Builds and sends the request at `key`; `adjust` can change it before sending.
    pub async fn send(&mut self, key: &str, adjust: impl FnOnce(&mut Request)) -> RequestRun {
        self.send_spec(SendSpec::request(key), adjust).await
    }

    /// Builds and sends `spec`: assertions are evaluated and captured values become
    /// runtime variables (secret ones are hidden from then on).
    pub async fn send_spec(
        &mut self,
        spec: SendSpec<'_>,
        adjust: impl FnOnce(&mut Request),
    ) -> RequestRun {
        let key = match spec.target {
            Target::Request(k) | Target::Inline { key: k, .. } => k.to_owned(),
        };
        if let Some(p) = &self.progress {
            p(&Progress::RequestStarted {
                iteration: self.iteration,
                key: key.clone(),
            });
        }
        let run = self.send_spec_inner(spec, adjust).await;
        if let Some(p) = &self.progress {
            let resp = run.response();
            p(&Progress::RequestDone {
                iteration: self.iteration,
                key,
                passed: run.passed(),
                status: resp.map(|r| r.status),
                duration_ms: resp.map(|r| r.timings.total.as_secs_f64() * 1000.0),
            });
        }
        run
    }

    async fn send_spec_inner(
        &mut self,
        spec: SendSpec<'_>,
        adjust: impl FnOnce(&mut Request),
    ) -> RequestRun {
        let (key, file, path) = match spec.target {
            Target::Request(key) => match self.collection.find(key) {
                Some(item) => (key, &item.file, self.collection.root.join(&item.path)),
                None => {
                    return RequestRun::invalid(
                        key,
                        None,
                        format!("no request `{key}` in the collection"),
                    );
                }
            },
            Target::Inline { key, file, path } => (key, file, path.to_path_buf()),
        };
        if file.kind() != snapreq_collection::RequestKind::Http {
            return RequestRun::invalid(
                key,
                None,
                format!(
                    "`{key}` is a {} request; runs and flows send HTTP requests only",
                    file.kind().as_str()
                ),
            );
        }
        let Built {
            mut request,
            scopes,
            changed,
            mut logs,
            test_codes,
        } = match self.build_for(&spec, key, file, &path).await {
            Ok(b) => b,
            Err(run) => return *run,
        };
        adjust(&mut request);

        let compile = |spec: &AssertSpec, file: &Path| -> Result<Assertions, String> {
            let spec = interpolate_assertions(spec, file, &scopes).map_err(|e| e.to_string())?;
            Assertions::compile(&spec)
        };
        let mut checks = vec![(&*file.assert, path.clone())];
        if let Some((a, p)) = spec.assert {
            checks.push((a, p.to_path_buf()));
        }
        let mut compiled = Vec::new();
        for (a, p) in checks {
            match compile(a, &p) {
                Ok(c) => compiled.push(c),
                Err(e) => return RequestRun::invalid(key, Some(request), e),
            }
        }
        let until = match spec.until.map(|(a, p)| compile(a, p)).transpose() {
            Ok(u) => u,
            Err(e) => return RequestRun::invalid(key, Some(request), e),
        };
        let mut capturers = Vec::new();
        for c in [Some(&file.capture), spec.capture].into_iter().flatten() {
            match Capturer::compile(c) {
                Ok(c) => capturers.push(c),
                Err(e) => return RequestRun::invalid(key, Some(request), e),
            }
        }

        let resp = match self.client.execute_with(&request, self.trace.clone()).await {
            Ok(resp) => resp,
            Err(e) => {
                return RequestRun {
                    key: key.to_owned(),
                    request: Some(request),
                    outcome: RunOutcome::Failed(e),
                    assertions: Vec::new(),
                    until: Vec::new(),
                    captures: Vec::new(),
                    logs,
                };
            }
        };
        let mut assertions: Vec<Outcome> =
            compiled.iter().flat_map(|c| c.evaluate(&resp)).collect();
        let until = until.map(|u| u.evaluate(&resp)).unwrap_or_default();
        let captures: Vec<Captured> = capturers.iter().flat_map(|c| c.extract(&resp)).collect();
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
        if !test_codes.is_empty() {
            // Test scripts see the values captured from this response.
            let scopes = self.scopes_with(key, spec.lower_vars, spec.upper_vars);
            let shown = changed.as_ref().unwrap_or(file);
            let chain = script::run_async(
                test_codes,
                script::input(
                    Phase::Test,
                    &scopes,
                    script::script_request(shown),
                    Some(script::script_response(&resp)),
                ),
            )
            .await;
            self.apply_script_vars(&chain.vars);
            let mut hidden = script::secret_values(&scopes);
            hidden.extend(request.secrets());
            hidden.extend(self.hidden.iter().cloned());
            logs.extend(chain.logs.iter().map(|l| crate::report::hide(l, &hidden)));
            let mut outcomes = script::outcomes(&chain);
            crate::report::scrub_outcomes(&mut outcomes, &hidden);
            assertions.extend(outcomes);
        }
        RequestRun {
            key: key.to_owned(),
            request: Some(request),
            outcome: RunOutcome::Response(Box::new(resp)),
            assertions,
            until,
            captures,
            logs,
        }
    }

    /// Builds the WebSocket request `key` for opening: pre-request scripts run, and its saved
    /// messages are resolved with the same variables.
    pub async fn websocket(&mut self, key: &str) -> Result<WsPrepared, String> {
        let collection = self.collection;
        let item = collection
            .find(key)
            .ok_or_else(|| format!("no request `{key}` in the collection"))?;
        if item.file.kind() != snapreq_collection::RequestKind::WebSocket {
            return Err(format!("`{key}` is not a WebSocket request"));
        }
        let path = collection.root.join(&item.path);
        let built = match self
            .build_for(&SendSpec::request(key), key, &item.file, &path)
            .await
        {
            Ok(b) => b,
            Err(run) => {
                return Err(match run.outcome {
                    RunOutcome::Invalid(why) => why,
                    _ => "the request could not be built".into(),
                });
            }
        };
        let file = built.changed.as_ref().unwrap_or(&item.file);
        let spec = file.websocket.clone().unwrap_or_default();
        let (messages, secrets) =
            snapreq_collection::websocket_messages(&spec, &path, &built.scopes)
                .map_err(|e| e.to_string())?;
        let mut hidden = built.request.secrets();
        hidden.extend(secrets);
        hidden.extend(script::secret_values(&built.scopes));
        Ok(WsPrepared {
            request: built.request,
            options: snapreq_core::client::ws::WsOptions {
                subprotocols: spec.subprotocols,
            },
            messages,
            logs: built.logs,
            hidden,
        })
    }

    /// Runs pre-request scripts and builds the request of `spec` (the first half of a send).
    async fn build_for(
        &mut self,
        spec: &SendSpec<'_>,
        key: &str,
        file: &RequestFile,
        path: &Path,
    ) -> Result<Built, Box<RequestRun>> {
        let path = path.to_path_buf();
        let mut scopes = self.scopes_with(key, spec.lower_vars, spec.upper_vars);
        let scripts = Scripts::for_request(self.collection, key, file, &path, spec.script);
        let (pre_codes, test_codes) =
            match (scripts.codes(Phase::PreRequest), scripts.codes(Phase::Test)) {
                (Ok(p), Ok(t)) => (p, t),
                (Err(e), _) | (_, Err(e)) => {
                    return Err(Box::new(RequestRun::invalid(key, None, e)));
                }
            };
        let mut logs = Vec::new();
        let mut changed: Option<RequestFile> = None;
        if !pre_codes.is_empty() {
            let before = script::script_request(file);
            let chain = script::run_async(
                pre_codes,
                script::input(Phase::PreRequest, &scopes, before.clone(), None),
            )
            .await;
            let mut hidden = script::secret_values(&scopes);
            self.apply_script_vars(&chain.vars);
            hidden.extend(self.hidden.iter().cloned());
            logs.extend(chain.logs.iter().map(|l| crate::report::hide(l, &hidden)));
            if let Some((label, e)) = &chain.error {
                return Err(Box::new(
                    RequestRun::invalid(
                        key,
                        None,
                        crate::report::hide(&format!("{label} pre-request script: {e}"), &hidden),
                    )
                    .with_logs(logs),
                ));
            }
            if let Some(after) = &chain.request
                && *after != before
            {
                let mut f = file.clone();
                script::apply_request(&mut f, &before, after);
                if let Err(e) = f.validate() {
                    return Err(Box::new(
                        RequestRun::invalid(
                            key,
                            None,
                            format!("after the pre-request script: {e}"),
                        )
                        .with_logs(logs),
                    ));
                }
                changed = Some(f);
            }
            scopes = self.scopes_with(key, spec.lower_vars, spec.upper_vars);
        }
        let built = match (spec.target, &changed) {
            (Target::Request(key), None) => build_request(self.collection, key, &scopes),
            (_, Some(f)) => build_file(self.collection, key, f, &path, &scopes),
            (Target::Inline { .. }, None) => build_file(self.collection, key, file, &path, &scopes),
        };
        let mut request = match built {
            Ok(r) => r,
            Err(e) => {
                return Err(Box::new(
                    RequestRun::invalid(key, None, e.to_string()).with_logs(logs),
                ));
            }
        };
        for v in &self.hidden {
            if !request.settings.secrets.contains(v) {
                request.settings.secrets.push(v.clone());
            }
        }
        Ok(Built {
            request,
            scopes,
            changed,
            logs,
            test_codes,
        })
    }

    /// Variables set by scripts become runtime variables, like captured values.
    fn apply_script_vars(&mut self, vars: &[snapreq_script::VarSet]) {
        for v in vars {
            if v.secret {
                self.runtime.set_secret(&v.name, &v.value);
                self.hide(v.value.clone());
            } else {
                self.runtime.set(&v.name, &v.value);
            }
        }
    }
}
