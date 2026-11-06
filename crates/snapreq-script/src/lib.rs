//! Pre-request and test scripts (PLAN P9.3, ADR-031): JavaScript run in a sandboxed QuickJS
//! engine with a small `snapreq.*` API.
//!
//! The sandbox is the engine's core language only: no modules, no file, network, process or timer
//! access. Every run gets a fresh runtime with a time limit (an interrupt handler), a memory
//! limit and a stack limit. The API (`prelude.js`) is plain JavaScript over the input document:
//! scripts read variables and the request or response, set variables, change the request
//! (pre-request) and record tests (`snapreq.test`, `snapreq.expect`).

use std::time::{Duration, Instant};

use rquickjs::{Context, Function, Runtime};
use serde::{Deserialize, Serialize};

const PRELUDE: &str = include_str!("prelude.js");

/// Errors that stop a script before it produced a result.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("the script ran longer than {0} ms")]
    Timeout(u128),
    #[error("the script engine failed: {0}")]
    Engine(String),
}

/// Resource limits of one script run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    pub time: Duration,
    /// Bytes the engine may allocate.
    pub memory: usize,
    /// Bytes of native stack the engine may use.
    pub stack: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            time: Duration::from_secs(2),
            memory: 32 << 20,
            stack: 512 << 10,
        }
    }
}

/// When a script runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Before the request is built: it may change variables and the request.
    PreRequest,
    /// After the response: tests and variables for later requests.
    Test,
}

/// A header as scripts see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Header {
    pub name: String,
    pub value: String,
}

/// The request as scripts see it (before variables are filled in).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptRequest {
    pub method: String,
    pub url: String,
    pub headers: Vec<Header>,
    /// Text of a text or JSON body; `None` for others.
    pub body: Option<String>,
}

/// The response as test scripts see it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScriptResponse {
    pub status: u16,
    pub headers: Vec<Header>,
    /// The body as text (lossy for binary bodies).
    pub body: String,
    pub time_ms: f64,
}

/// What a script is given.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Input {
    pub phase: Phase,
    /// Variables the request sees, name → value (secret values included: scripts may need them).
    pub vars: serde_json::Map<String, serde_json::Value>,
    pub request: ScriptRequest,
    pub response: Option<ScriptResponse>,
}

/// A variable a script set.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VarSet {
    pub name: String,
    pub value: String,
    pub secret: bool,
}

/// One `snapreq.test(...)`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TestResult {
    pub name: String,
    pub passed: bool,
    pub error: Option<String>,
}

/// What a script did.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Output {
    /// Variables set, in order (later sets of a name win).
    pub vars: Vec<VarSet>,
    /// The request after the script (only pre-request scripts can change it).
    pub request: ScriptRequest,
    pub tests: Vec<TestResult>,
    /// `console.log` lines.
    pub logs: Vec<String>,
    /// An uncaught exception (the script stopped there; what it did before still counts).
    pub error: Option<String>,
}

/// Runs `code` with `input` within `limits`.
pub fn run(code: &str, input: &Input, limits: &Limits) -> Result<Output, Error> {
    let engine = |e: rquickjs::Error| Error::Engine(e.to_string());
    let rt = Runtime::new().map_err(engine)?;
    rt.set_memory_limit(limits.memory);
    rt.set_max_stack_size(limits.stack);
    let deadline = Instant::now() + limits.time;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let ctx = Context::full(&rt).map_err(engine)?;
    let input_json = serde_json::to_string(input).map_err(|e| Error::Engine(e.to_string()))?;
    let result: Result<String, String> = ctx.with(|ctx| {
        let describe = |ctx: &rquickjs::Ctx<'_>, e: rquickjs::Error| -> String {
            if matches!(e, rquickjs::Error::Exception) {
                let caught = ctx.catch();
                caught
                    .as_exception()
                    .and_then(rquickjs::Exception::message)
                    .or_else(|| caught.as_string().and_then(|s| s.to_string().ok()))
                    .unwrap_or_else(|| "exception".into())
            } else {
                e.to_string()
            }
        };
        ctx.globals()
            .set("__snapreq_input", input_json)
            .map_err(|e| describe(&ctx, e))?;
        ctx.eval::<(), _>(PRELUDE).map_err(|e| describe(&ctx, e))?;
        let run: Function = ctx
            .globals()
            .get("__snapreq_run")
            .map_err(|e| describe(&ctx, e))?;
        run.call::<_, String>((code,))
            .map_err(|e| describe(&ctx, e))
    });
    if Instant::now() >= deadline {
        return Err(Error::Timeout(limits.time.as_millis()));
    }
    let text = result.map_err(Error::Engine)?;
    serde_json::from_str(&text).map_err(|e| Error::Engine(format!("unreadable script result: {e}")))
}

/// What a chain of scripts (collection, folders, request, step) did together.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Chain {
    pub vars: Vec<VarSet>,
    /// The request after the last script (unchanged in the test phase).
    pub request: Option<ScriptRequest>,
    /// Tests with the label of the script that recorded them.
    pub tests: Vec<(String, TestResult)>,
    pub logs: Vec<String>,
    /// The label of the script that failed and why; later scripts did not run.
    pub error: Option<(String, String)>,
}

/// Runs `scripts` (`(label, code)`) one after another: each sees the variables set and (before
/// the request) the request changed by those before it. Stops at the first error.
pub fn run_chain(scripts: &[(String, String)], mut input: Input, limits: &Limits) -> Chain {
    let mut chain = Chain::default();
    for (label, code) in scripts {
        match run(code, &input, limits) {
            Ok(out) => {
                for v in &out.vars {
                    input
                        .vars
                        .insert(v.name.clone(), serde_json::Value::String(v.value.clone()));
                }
                chain.vars.extend(out.vars);
                chain
                    .tests
                    .extend(out.tests.into_iter().map(|t| (label.clone(), t)));
                chain.logs.extend(out.logs);
                if input.phase == Phase::PreRequest {
                    input.request = out.request;
                }
                if let Some(e) = out.error {
                    chain.error = Some((label.clone(), e));
                    break;
                }
            }
            Err(e) => {
                chain.error = Some((label.clone(), e.to_string()));
                break;
            }
        }
    }
    chain.request = Some(input.request);
    chain
}

/// Whether `code` is a valid script, without running it (it is only compiled).
pub fn check_syntax(code: &str) -> Result<(), String> {
    let rt = Runtime::new().map_err(|e| e.to_string())?;
    rt.set_memory_limit(Limits::default().memory);
    let deadline = Instant::now() + Duration::from_millis(500);
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));
    let ctx = Context::full(&rt).map_err(|e| e.to_string())?;
    ctx.with(|ctx| {
        ctx.globals()
            .set("__snapreq_code", code)
            .map_err(|e| e.to_string())?;
        match ctx.eval::<(), _>(
            "new Function(\"snapreq\", \"console\", '\"use strict\";\\n' + globalThis.__snapreq_code);",
        ) {
            Ok(()) => Ok(()),
            Err(rquickjs::Error::Exception) => {
                let caught = ctx.catch();
                Err(caught
                    .as_exception()
                    .and_then(rquickjs::Exception::message)
                    .unwrap_or_else(|| "invalid script".into()))
            }
            Err(e) => Err(e.to_string()),
        }
    })
}
