//! Running a request's scripts (PLAN P9.3, ADR-031): the collection's, each enclosing folder's,
//! the request's and a flow step's `[script]`, in that order.

use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use snapreq_collection::{BodySpec, Collection, RequestFile, Scopes, ScriptSpec};
use snapreq_core::Response;
use snapreq_core::model::KeyValue;
use snapreq_script::{Chain, Header, Input, Limits, Phase, ScriptRequest, ScriptResponse};

use crate::assert::Outcome;

/// The scripts that apply to one request, outermost first: `(label, spec, directory)`.
#[derive(Debug, Clone, Default)]
pub struct Scripts(Vec<(String, ScriptSpec, PathBuf)>);

impl Scripts {
    /// The scripts of the request at `key` (built from `file` at `file_path`), plus a step's.
    pub fn for_request(
        collection: &Collection,
        key: &str,
        file: &RequestFile,
        file_path: &Path,
        step: Option<(&ScriptSpec, &Path)>,
    ) -> Self {
        let mut out = Vec::new();
        let root = &collection.root;
        if !collection.file.script.is_empty() {
            out.push((
                "collection".to_owned(),
                collection.file.script.clone(),
                root.clone(),
            ));
        }
        for folder in collection.folders_of(key) {
            if !folder.file.script.is_empty() {
                out.push((
                    format!("folder {}", folder.path.display()),
                    folder.file.script.clone(),
                    root.join(&folder.path),
                ));
            }
        }
        let dir = |p: &Path| p.parent().unwrap_or(root).to_path_buf();
        if !file.script.is_empty() {
            out.push(("request".to_owned(), file.script.clone(), dir(file_path)));
        }
        if let Some((spec, path)) = step
            && !spec.is_empty()
        {
            out.push(("step".to_owned(), spec.clone(), dir(path)));
        }
        Self(out)
    }

    /// `(label, code)` of one phase; a script file that cannot be read is an error.
    pub fn codes(&self, phase: Phase) -> Result<Vec<(String, String)>, String> {
        let mut out = Vec::new();
        for (label, spec, dir) in &self.0 {
            if let Some(code) = spec
                .code(phase == Phase::Test, dir)
                .map_err(|e| format!("{label} script: {e}"))?
            {
                out.push((label.clone(), code));
            }
        }
        Ok(out)
    }
}

/// Every variable the request sees, by name (secret values included).
pub fn vars_of(scopes: &Scopes) -> Map<String, Value> {
    scopes
        .visible()
        .into_iter()
        .filter_map(|(v, _)| Some((v.name.clone(), Value::String(v.value.clone()?))))
        .collect()
}

/// Values of secret variables, to hide in what scripts print.
pub fn secret_values(scopes: &Scopes) -> Vec<String> {
    scopes
        .visible()
        .into_iter()
        .filter(|(v, _)| v.secret || scopes.is_declared_secret(&v.name))
        .filter_map(|(v, _)| v.value.clone())
        .filter(|v| !v.is_empty())
        .collect()
}

/// The request file as scripts see it (before variables are filled in).
pub fn script_request(file: &RequestFile) -> ScriptRequest {
    ScriptRequest {
        method: file.method().to_owned(),
        url: file.url.clone(),
        headers: file
            .headers
            .0
            .iter()
            .filter(|h| h.enabled)
            .map(|h| Header {
                name: h.name.clone(),
                value: h.value.clone(),
            })
            .collect(),
        body: match &file.body {
            Some(BodySpec::Text {
                content: Some(c), ..
            })
            | Some(BodySpec::Json {
                content: Some(c), ..
            }) => Some(c.clone()),
            _ => None,
        },
    }
}

/// Applies what a pre-request script changed to the request file.
pub fn apply_request(file: &mut RequestFile, before: &ScriptRequest, after: &ScriptRequest) {
    if after.method != before.method {
        file.method = (!after.method.eq_ignore_ascii_case("GET")).then(|| after.method.clone());
    }
    if after.url != before.url {
        file.url.clone_from(&after.url);
    }
    if after.headers != before.headers {
        let disabled: Vec<KeyValue> = file
            .headers
            .0
            .iter()
            .filter(|h| !h.enabled)
            .cloned()
            .collect();
        file.headers.0 = after
            .headers
            .iter()
            .map(|h| KeyValue::new(&h.name, &h.value))
            .chain(disabled)
            .collect();
    }
    if after.body != before.body {
        file.body = after.body.as_ref().map(|text| match &file.body {
            Some(BodySpec::Text { content_type, .. }) => BodySpec::Text {
                content: Some(text.clone()),
                file: None,
                content_type: content_type.clone(),
            },
            Some(BodySpec::Json { .. }) => BodySpec::Json {
                content: Some(text.clone()),
                file: None,
            },
            _ if serde_json::from_str::<Value>(text)
                .is_ok_and(|v| v.is_object() || v.is_array()) =>
            {
                BodySpec::Json {
                    content: Some(text.clone()),
                    file: None,
                }
            }
            _ => BodySpec::Text {
                content: Some(text.clone()),
                file: None,
                content_type: None,
            },
        });
    }
}

/// The response as test scripts see it.
pub fn script_response(resp: &Response) -> ScriptResponse {
    ScriptResponse {
        status: resp.status,
        headers: resp
            .headers
            .iter()
            .map(|h| Header {
                name: h.name.clone(),
                value: h.value.clone(),
            })
            .collect(),
        body: String::from_utf8_lossy(&resp.body).into_owned(),
        time_ms: resp.timings.total.as_secs_f64() * 1000.0,
    }
}

/// Runs a chain of scripts (blocking: call from a blocking context).
pub fn run(codes: &[(String, String)], input: Input) -> Chain {
    snapreq_script::run_chain(codes, input, &Limits::default())
}

/// [`run`] on a blocking thread.
pub async fn run_async(codes: Vec<(String, String)>, input: Input) -> Chain {
    tokio::task::spawn_blocking(move || run(&codes, input))
        .await
        .unwrap_or_else(|e| Chain {
            error: Some(("script".into(), e.to_string())),
            ..Chain::default()
        })
}

/// Test results (and a failed script) as check outcomes: `test <name>`.
pub fn outcomes(chain: &Chain) -> Vec<Outcome> {
    let mut out: Vec<Outcome> = chain
        .tests
        .iter()
        .map(|(_, t)| Outcome {
            subject: "test".into(),
            check: t.name.clone(),
            passed: t.passed,
            actual: None,
            message: t.error.clone(),
        })
        .collect();
    if let Some((label, e)) = &chain.error {
        out.push(Outcome {
            subject: "script".into(),
            check: format!("{label} test script runs"),
            passed: false,
            actual: None,
            message: Some(e.clone()),
        });
    }
    out
}

/// A script input.
pub fn input(
    phase: Phase,
    scopes: &Scopes,
    request: ScriptRequest,
    response: Option<ScriptResponse>,
) -> Input {
    Input {
        phase,
        vars: vars_of(scopes),
        request,
        response,
    }
}
