//! `snapreq sse`: read a Server-Sent Events stream and print events as they arrive (PLAN P10.2).

use std::io::Write;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::json;
use snapreq_core::model::KeyValue;
use snapreq_core::sse::{self, TimedEvent};
use snapreq_core::{Error, Streaming};

use crate::cli::{FormatArg, SseArgs};
use crate::collection_cmd::{self, Target};
use crate::{exit, trace_view, waterfall};

/// How the stream ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum End {
    Server,
    MaxEvents,
    Duration,
    Interrupted,
}

impl End {
    fn as_str(self) -> &'static str {
        match self {
            Self::Server => "server",
            Self::MaxEvents => "max_events",
            Self::Duration => "duration",
            Self::Interrupted => "interrupted",
        }
    }
}

fn print_event(e: &TimedEvent, json: bool, hidden: &[String]) {
    let hide = |t: &str| snapreq_test::report::hide(t, hidden);
    let mut out = std::io::stdout().lock();
    if json {
        let mut doc = json!(e);
        doc["data"] = json!(hide(&e.event.data));
        let _ = writeln!(out, "{doc}");
        return;
    }
    let mut text = String::new();
    if let Some(id) = &e.event.id {
        text.push_str(&format!("id: {}\n", hide(id)));
    }
    if e.event.event != "message" {
        text.push_str(&format!("event: {}\n", e.event.event));
    }
    for line in hide(&e.event.data).split('\n') {
        text.push_str(&format!("data: {line}\n"));
    }
    text.push('\n');
    let _ = out.write_all(text.as_bytes());
    let _ = out.flush();
}

pub(crate) async fn run(args: &SseArgs) -> u8 {
    let r = &args.request;
    let json = r.format == FormatArg::Json;
    let invalid = |message: String| {
        if json {
            crate::emit_json(&crate::json_out::invalid_arguments(&message));
        } else {
            eprintln!("error: {message}");
        }
        exit::INVALID_REQUEST
    };
    if r.export.is_some() || r.print.is_some() {
        return invalid("--export and --print do not apply to `snapreq sse`".into());
    }
    let built = collection_cmd::resolve_target(r).and_then(|target| match target {
        Target::Url => crate::send::build_request(r).map(|req| (req, Default::default())),
        Target::Collection { root, key } => {
            collection_cmd::build(&root, &key, r).map(|b| (b.0, b.1))
        }
    });
    let (mut request, checks) = match built {
        Ok(b) => b,
        Err(e) => return invalid(e.to_string()),
    };
    let assertions = match snapreq_test::Assertions::compile(&checks) {
        Ok(a) => a,
        Err(e) => return invalid(e),
    };
    let has = |req: &snapreq_core::Request, name: &str| {
        req.enabled_headers()
            .any(|h| h.name.eq_ignore_ascii_case(name))
    };
    if !has(&request, "accept") {
        request
            .headers
            .push(KeyValue::new("Accept", "text/event-stream"));
    }
    if let Some(id) = &args.last_event_id {
        request.headers.push(KeyValue::new("Last-Event-ID", id));
    }
    if let Err(e) = request.effective_url() {
        return invalid(e.to_string());
    }

    let hidden = request.secrets();
    let count = Arc::new(AtomicUsize::new(0));
    let filter = args.event.clone();
    let max = args.max_events;
    let mut streaming = Streaming::default();
    let stop = streaming.stop.clone();
    let end = Arc::new(std::sync::Mutex::new(End::Server));
    {
        let (count, stop, end, hidden) = (count.clone(), stop.clone(), end.clone(), hidden.clone());
        streaming.observer = Some(sse::observer(move |e| {
            if !filter.is_empty() && !filter.contains(&e.event.event) {
                return;
            }
            print_event(e, json, &hidden);
            let n = count.fetch_add(1, Ordering::SeqCst) + 1;
            if max.is_some_and(|m| n >= m) {
                *end.lock().unwrap_or_else(|p| p.into_inner()) = End::MaxEvents;
                stop.cancel();
            }
        }));
    }
    if let Some(secs) = args.duration {
        let (stop, end) = (stop.clone(), end.clone());
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs_f64(secs.max(0.0))).await;
            if !stop.is_cancelled() {
                *end.lock().unwrap_or_else(|p| p.into_inner()) = End::Duration;
                stop.cancel();
            }
        });
    }
    {
        let (stop, end) = (stop.clone(), end.clone());
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                *end.lock().unwrap_or_else(|p| p.into_inner()) = End::Interrupted;
                stop.cancel();
            }
        });
    }

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
            let _ = waterfall::render(
                &mut err,
                trace,
                waterfall::terminal_width(waterfall::Stream::Stderr),
            );
        }
    };

    let result = crate::client()
        .execute_streaming(&request, None, Some(streaming))
        .await;
    let response = match result {
        Ok(r) => r,
        Err(e) => {
            if let Some(t) = e.trace() {
                show_trace(t);
            }
            if json {
                crate::emit_json(&crate::json_out::failure(Some(&request), &e));
            } else {
                eprintln!("error: {e}");
            }
            return match e {
                Error::Execution(ex) if ex.timed_out => exit::TIMEOUT,
                Error::Execution(_) => exit::NETWORK,
                _ => exit::INVALID_REQUEST,
            };
        }
    };
    show_trace(&response.trace);
    let end = *end.lock().unwrap_or_else(|p| p.into_inner());
    let events = count.load(Ordering::SeqCst);
    let content_type = response
        .header("content-type")
        .unwrap_or_default()
        .to_owned();
    if response.status >= 400 || !sse::is_event_stream(&content_type) {
        let body = snapreq_test::report::hide(&String::from_utf8_lossy(&response.body), &hidden);
        if json {
            crate::emit_json(&json!({
                "end": "not_an_event_stream",
                "status": response.status,
                "content_type": content_type,
                "body": body,
            }));
        } else {
            eprintln!(
                "error: the server answered {} with {} instead of an event stream",
                response.status,
                if content_type.is_empty() {
                    "no content type"
                } else {
                    content_type.as_str()
                }
            );
            if !body.trim().is_empty() {
                println!("{}", body.trim_end());
            }
        }
        return if response.status >= 400 {
            exit::HTTP_ERROR
        } else {
            exit::NETWORK
        };
    }
    let outcomes = assertions.evaluate(&response);
    if json {
        let mut doc = json!({ "end": end.as_str(), "status": response.status, "events": events });
        if !outcomes.is_empty() {
            doc["assertions"] = serde_json::to_value(&outcomes).unwrap_or_default();
        }
        let _ = writeln!(std::io::stdout().lock(), "{doc}");
    } else {
        let why = match end {
            End::Server => "the server ended the stream",
            End::MaxEvents => "reached --max-events",
            End::Duration => "reached --duration",
            End::Interrupted => "interrupted",
        };
        eprintln!("{events} event(s); {why}");
        if !outcomes.is_empty() {
            let mut err = anstream::AutoStream::new(std::io::stderr().lock(), color);
            let _ = crate::assert_view::render(&mut err, &outcomes);
        }
    }
    if outcomes.iter().any(|o| !o.passed) {
        exit::ASSERTION_FAILED
    } else {
        exit::OK
    }
}
