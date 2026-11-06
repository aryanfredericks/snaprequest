//! `snapreq grpc`: call gRPC methods, list and describe services (PLAN P10.4).

use std::io::Write;
use std::path::PathBuf;

use serde_json::{Value, json};
use snapreq_grpc::{CallEvent, DescriptorPool};

use crate::cli::{FormatArg, GrpcArgs};
use crate::collection_cmd::{self, Target};
use crate::send::ArgError;
use crate::{exit, trace_view};

/// What to call, from the arguments and (for collection requests) the file.
struct Plan {
    request: snapreq_core::Request,
    method: Option<String>,
    protos: Vec<PathBuf>,
    import_paths: Vec<PathBuf>,
    messages: Vec<String>,
    hidden: Vec<String>,
}

fn read(value: &str) -> Result<String, ArgError> {
    match value.strip_prefix('@') {
        Some(path) => std::fs::read_to_string(path)
            .map_err(|e| ArgError(format!("cannot read `{path}`: {e}"))),
        None => Ok(value.to_owned()),
    }
}

fn plan(args: &GrpcArgs) -> Result<Plan, ArgError> {
    let r = &args.request;
    if r.json.is_some()
        || !r.form.is_empty()
        || !r.multipart.is_empty()
        || r.file.is_some()
        || r.graphql.is_some()
    {
        return Err(ArgError("gRPC messages are given with -m JSON".into()));
    }
    if r.export.is_some() || r.output.is_some() || r.print.is_some() {
        return Err(ArgError(
            "--export, --output and --print do not apply to `snapreq grpc`".into(),
        ));
    }
    let mut messages: Vec<String> = args
        .messages
        .iter()
        .map(|m| read(m))
        .collect::<Result<_, _>>()?;
    if let Some(d) = &r.data {
        messages.push(read(d)?);
    }
    let mut plan = match collection_cmd::resolve_target(r)? {
        Target::Url => {
            let (url, method) = match r.target.as_slice() {
                [url] => (url.clone(), args.rpc.clone()),
                [url, method] => (url.clone(), Some(method.clone())),
                _ => {
                    return Err(ArgError(
                        "expected `HOST:PORT [package.Service/Method]`".into(),
                    ));
                }
            };
            let mut req = snapreq_core::Request::get(url);
            // A `-d` body is taken as a message above; the request itself has no body.
            crate::send::apply_overrides(&mut req, r)?;
            req.body = snapreq_core::Body::None;
            Plan {
                request: req,
                method,
                protos: Vec::new(),
                import_paths: Vec::new(),
                messages: Vec::new(),
                hidden: Vec::new(),
            }
        }
        Target::Collection { root, key } => {
            let full = collection_cmd::build_full(&root, &key, r)?;
            if full.file.kind() != snapreq_collection::RequestKind::Grpc {
                return Err(ArgError(format!(
                    "`{key}` is not a gRPC request (give it a grpc:// URL or a [grpc] table)"
                )));
            }
            let spec = full.file.grpc.clone().unwrap_or_default();
            let (call, secrets) = snapreq_collection::grpc_call(&spec, &full.path, &full.scopes)
                .map_err(|e| ArgError(e.to_string()))?;
            Plan {
                request: full.built.0,
                method: args.rpc.clone().or(call.method),
                protos: call.protos,
                import_paths: call.import_paths,
                messages: call.messages,
                hidden: secrets,
            }
        }
    };
    if !args.protos.is_empty() {
        plan.protos = args.protos.clone();
    }
    plan.import_paths.extend(args.import_paths.iter().cloned());
    if !messages.is_empty() {
        plan.messages = messages;
    }
    plan.hidden.extend(plan.request.secrets());
    Ok(plan)
}

fn print_json(v: &Value, hidden: &[String]) {
    let v = snapreq_test::report::hide_json(v.clone(), hidden);
    let mut out = std::io::stdout().lock();
    let _ = writeln!(
        out,
        "{}",
        serde_json::to_string_pretty(&v).unwrap_or_default()
    );
}

fn list(pool: &DescriptorPool) -> String {
    let mut out = String::new();
    for s in snapreq_grpc::describe(pool) {
        out.push_str(&format!("{}\n", s.name));
        for m in &s.methods {
            let short = |n: &str| n.rsplit('.').next().unwrap_or(n).to_owned();
            out.push_str(&format!(
                "  {}({}{}) returns ({}{})\n",
                m.name,
                if m.client_streaming { "stream " } else { "" },
                short(&m.input),
                if m.server_streaming { "stream " } else { "" },
                short(&m.output),
            ));
        }
    }
    out
}

fn describe_name(pool: &DescriptorPool, name: &str) -> Result<Value, String> {
    if let Some(svc) = snapreq_grpc::describe(pool)
        .into_iter()
        .find(|s| s.name == name)
    {
        return Ok(json!(svc));
    }
    if let Some(m) = pool.get_message_by_name(name) {
        return Ok(
            json!({ "message": m.full_name(), "template": snapreq_grpc::message_template(&m) }),
        );
    }
    let method = snapreq_grpc::find_method(pool, name).map_err(|e| e.to_string())?;
    let svc = method.parent_service().full_name().to_owned();
    let info = snapreq_grpc::describe(pool)
        .into_iter()
        .find(|s| s.name == svc)
        .and_then(|s| s.methods.into_iter().find(|m| m.name == method.name()))
        .ok_or("method not found")?;
    Ok(
        json!({ "method": info.path, "kind": info.kind(), "input": info.input, "output": info.output, "template": info.template }),
    )
}

pub(crate) async fn run(args: &GrpcArgs) -> u8 {
    let r = &args.request;
    let json_out = r.format == FormatArg::Json;
    let fail = |message: String, code: u8| {
        if json_out {
            crate::emit_json(&json!({ "error": { "message": message } }));
        } else {
            eprintln!("error: {message}");
        }
        code
    };
    let plan = match plan(args) {
        Ok(p) => p,
        Err(e) => return fail(e.to_string(), exit::INVALID_REQUEST),
    };
    let hide = |t: &str| snapreq_test::report::hide(t, &plan.hidden);
    let color = match r.color {
        crate::cli::ColorArg::Auto => anstream::ColorChoice::Auto,
        crate::cli::ColorArg::Always => anstream::ColorChoice::Always,
        crate::cli::ColorArg::Never => anstream::ColorChoice::Never,
    };
    let show_trace = |trace: &[snapreq_core::model::TraceEntry]| {
        if r.verbose > 0 {
            let mut err = anstream::AutoStream::new(std::io::stderr().lock(), color);
            let _ = trace_view::render(&mut err, trace, r.verbose);
            let _ = std::io::Write::write_all(&mut err, b"\n");
        }
    };
    let channel = match snapreq_grpc::connect(&crate::client(), &plan.request, None).await {
        Ok(c) => c,
        Err(snapreq_grpc::Error::Core(e)) => {
            if let Some(t) = e.trace() {
                show_trace(t);
            }
            let code = match &e {
                snapreq_core::Error::Execution(ex) if ex.timed_out => exit::TIMEOUT,
                snapreq_core::Error::Execution(_) => exit::NETWORK,
                _ => exit::INVALID_REQUEST,
            };
            return fail(hide(&e.to_string()), code);
        }
        Err(e) => return fail(hide(&e.to_string()), exit::INVALID_REQUEST),
    };
    show_trace(&channel.trace);
    let pool = if plan.protos.is_empty() {
        snapreq_grpc::reflect(&channel).await
    } else {
        snapreq_grpc::compile_protos(&plan.protos, &plan.import_paths)
    };
    let pool = match pool {
        Ok(p) => p,
        Err(e) => return fail(hide(&e.to_string()), exit::NETWORK),
    };
    if args.list {
        if json_out {
            crate::emit_json(&json!(snapreq_grpc::describe(&pool)));
        } else {
            print!("{}", list(&pool));
        }
        return exit::OK;
    }
    if let Some(name) = &args.describe {
        return match describe_name(&pool, name) {
            Ok(v) => {
                crate::emit_json(&v);
                exit::OK
            }
            Err(e) => fail(e, exit::INVALID_REQUEST),
        };
    }
    let Some(method_name) = &plan.method else {
        return fail(
            "name the method to call (package.Service/Method), or use --list".into(),
            exit::INVALID_REQUEST,
        );
    };
    let method = match snapreq_grpc::find_method(&pool, method_name) {
        Ok(m) => m,
        Err(e) => return fail(e.to_string(), exit::INVALID_REQUEST),
    };
    let mut texts = plan.messages.clone();
    if texts.is_empty() && !method.is_client_streaming() {
        texts.push("{}".into());
    }
    if texts.len() > 1 && !method.is_client_streaming() {
        return fail(
            format!(
                "`{method_name}` takes one message, {} were given",
                texts.len()
            ),
            exit::INVALID_REQUEST,
        );
    }
    let mut inputs = Vec::new();
    for t in &texts {
        match snapreq_grpc::message_from_json(&method.input(), t) {
            Ok(m) => inputs.push(m),
            Err(e) => return fail(hide(&e.to_string()), exit::INVALID_REQUEST),
        }
    }
    let verbose = r.verbose > 0;
    let hidden = plan.hidden.clone();
    let outcome =
        snapreq_grpc::call(
            &channel,
            &method,
            futures_util::stream::iter(inputs),
            |e| match e {
                CallEvent::Headers(h) if verbose && !json_out => {
                    eprintln!(
                        "headers: {}",
                        snapreq_test::report::hide(&h.to_string(), &hidden)
                    );
                }
                CallEvent::Message { message, .. } if !json_out => print_json(&message, &hidden),
                _ => {}
            },
        )
        .await;
    let outcome = match outcome {
        Ok(o) => o,
        Err(e) => return fail(hide(&e.to_string()), exit::NETWORK),
    };
    if json_out {
        let mut doc = json!(outcome);
        doc["method"] = json!(method_name);
        crate::emit_json(&snapreq_test::report::hide_json(doc, &plan.hidden));
    } else {
        if verbose {
            eprintln!("trailers: {}", hide(&outcome.trailers.to_string()));
        }
        let ms = outcome.duration.as_secs_f64() * 1000.0;
        if outcome.status.ok() {
            eprintln!("OK ({} message(s), {ms:.0} ms)", outcome.messages.len());
        } else {
            eprintln!(
                "error: {} ({}): {}",
                outcome.status.name,
                outcome.status.code,
                hide(&outcome.status.message)
            );
        }
    }
    if outcome.status.ok() {
        exit::OK
    } else {
        exit::HTTP_ERROR
    }
}
