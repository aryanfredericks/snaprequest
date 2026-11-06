//! `snapreq ws`: open a WebSocket, send messages and print what arrives (PLAN P10.1).

use std::io::{IsTerminal, Write};
use std::time::{Duration, Instant};

use serde_json::json;
use snapreq_core::Error;
use snapreq_core::client::ws::{WsMessage, WsOptions};
use tokio::io::AsyncBufReadExt;

use crate::cli::{FormatArg, WsArgs};
use crate::collection_cmd::{self, Target};
use crate::send::ArgError;
use crate::{exit, trace_view, waterfall};

/// Lines printed for a binary payload show at most this many bytes.
const HEX_PREVIEW: usize = 64;

struct Printer {
    json: bool,
    verbose: bool,
    hidden: Vec<String>,
    start: Instant,
}

impl Printer {
    fn hide(&self, text: &str) -> String {
        snapreq_test::report::hide(text, &self.hidden)
    }

    fn message(&self, outgoing: bool, m: &WsMessage) {
        let quiet = matches!(m, WsMessage::Ping { .. } | WsMessage::Pong { .. });
        if quiet && !self.verbose && !self.json {
            return;
        }
        let mut out = std::io::stdout().lock();
        if self.json {
            let mut doc = serde_json::to_value(m).unwrap_or_default();
            if let Some(data) = doc.get("data").and_then(|d| d.as_str()) {
                doc["data"] = json!(self.hide(data));
            }
            doc["direction"] = json!(if outgoing { "sent" } else { "received" });
            doc["at_ms"] = json!(self.start.elapsed().as_secs_f64() * 1000.0);
            let _ = writeln!(out, "{doc}");
            return;
        }
        let arrow = if outgoing { ">" } else { "<" };
        let line = match m {
            WsMessage::Text { data } => self.hide(data),
            WsMessage::Binary { data } => binary_line("binary", data),
            WsMessage::Ping { data } => binary_line("ping", data),
            WsMessage::Pong { data } => binary_line("pong", data),
            WsMessage::Close { code, reason } => {
                format!(
                    "[close {}] {}",
                    code.map_or("-".into(), |c| c.to_string()),
                    self.hide(reason)
                )
            }
        };
        let _ = writeln!(out, "{arrow} {line}");
    }

    fn event(&self, name: &str, text: &str, extra: serde_json::Value) {
        if self.json {
            let mut doc =
                json!({ "event": name, "at_ms": self.start.elapsed().as_secs_f64() * 1000.0 });
            if let (Some(d), Some(e)) = (doc.as_object_mut(), extra.as_object()) {
                d.extend(e.clone());
            }
            let _ = writeln!(std::io::stdout().lock(), "{doc}");
        } else {
            eprintln!("{}", self.hide(text));
        }
    }
}

fn binary_line(label: &str, data: &[u8]) -> String {
    let hex: Vec<String> = data
        .iter()
        .take(HEX_PREVIEW)
        .map(|b| format!("{b:02x}"))
        .collect();
    let more = if data.len() > HEX_PREVIEW { " …" } else { "" };
    format!("[{label} {} bytes] {}{more}", data.len(), hex.join(" "))
}

/// What to send and how to connect, from the arguments and (for collection requests) the file.
struct Plan {
    request: snapreq_core::Request,
    options: WsOptions,
    messages: Vec<WsMessage>,
    hidden: Vec<String>,
}

fn plan(args: &WsArgs) -> Result<Plan, ArgError> {
    let r = &args.request;
    if r.data.is_some()
        || r.json.is_some()
        || !r.form.is_empty()
        || !r.multipart.is_empty()
        || r.file.is_some()
    {
        return Err(ArgError(
            "a WebSocket has no request body; send messages with --send".into(),
        ));
    }
    if r.export.is_some() || r.output.is_some() || r.print.is_some() {
        return Err(ArgError(
            "--export, --output and --print do not apply to `snapreq ws`".into(),
        ));
    }
    let mut messages = Vec::new();
    let mut hidden = Vec::new();
    let (request, mut options) = match collection_cmd::resolve_target(r)? {
        Target::Url => {
            if !args.saved.is_empty() {
                return Err(ArgError(
                    "--message names a saved message of a collection request".into(),
                ));
            }
            let [url] = r.target.as_slice() else {
                return Err(ArgError(
                    "expected one URL such as ws://localhost:8080/chat".into(),
                ));
            };
            let url = if url.contains("://") {
                url.clone()
            } else {
                format!("ws://{url}")
            };
            let mut req = snapreq_core::Request::get(url);
            crate::send::apply_overrides(&mut req, r)?;
            (req, WsOptions::default())
        }
        Target::Collection { root, key } => {
            let full = collection_cmd::build_full(&root, &key, r)?;
            if full.file.kind() != snapreq_collection::RequestKind::WebSocket {
                return Err(ArgError(format!(
                    "`{key}` is not a WebSocket request (give it a ws:// URL or a [websocket] table); use `snapreq send`"
                )));
            }
            let spec = full.file.websocket.clone().unwrap_or_default();
            let (saved, secrets) =
                snapreq_collection::websocket_messages(&spec, &full.path, &full.scopes)
                    .map_err(|e| ArgError(e.to_string()))?;
            hidden.extend(secrets);
            for name in &args.saved {
                if name == "all" {
                    messages.extend(saved.iter().map(|(_, m)| m.clone()));
                    continue;
                }
                let found = saved.iter().find(|(n, _)| n == name).ok_or_else(|| {
                    let names: Vec<&str> = saved.iter().map(|(n, _)| n.as_str()).collect();
                    ArgError(format!(
                        "no saved message `{name}` in `{key}` (saved: {})",
                        if names.is_empty() {
                            "none".into()
                        } else {
                            names.join(", ")
                        }
                    ))
                })?;
                messages.push(found.1.clone());
            }
            (
                full.built.0,
                WsOptions {
                    subprotocols: spec.subprotocols,
                },
            )
        }
    };
    if !args.subprotocols.is_empty() {
        options.subprotocols = args.subprotocols.clone();
    }
    messages.extend(
        args.send
            .iter()
            .map(|t| WsMessage::Text { data: t.clone() }),
    );
    hidden.extend(request.secrets());
    Ok(Plan {
        request,
        options,
        messages,
        hidden,
    })
}

pub(crate) async fn run(args: &WsArgs) -> u8 {
    let json = args.request.format == FormatArg::Json;
    let plan = match plan(args) {
        Ok(p) => p,
        Err(e) => {
            if json {
                crate::emit_json(&crate::json_out::invalid_arguments(&e.to_string()));
            } else {
                eprintln!("error: {e}");
            }
            return exit::INVALID_REQUEST;
        }
    };
    let printer = Printer {
        json,
        verbose: args.request.verbose > 0,
        hidden: plan.hidden.clone(),
        start: Instant::now(),
    };
    let color = match args.request.color {
        crate::cli::ColorArg::Auto => anstream::ColorChoice::Auto,
        crate::cli::ColorArg::Always => anstream::ColorChoice::Always,
        crate::cli::ColorArg::Never => anstream::ColorChoice::Never,
    };
    let show_trace = |trace: &[snapreq_core::model::TraceEntry]| {
        if args.request.verbose > 0 {
            let mut err = anstream::AutoStream::new(std::io::stderr().lock(), color);
            let _ = trace_view::render(&mut err, trace, args.request.verbose);
            let _ = std::io::Write::write_all(&mut err, b"\n");
            let _ = waterfall::render(
                &mut err,
                trace,
                waterfall::terminal_width(waterfall::Stream::Stderr),
            );
        }
    };

    let ws = match crate::client()
        .websocket(&plan.request, &plan.options, None)
        .await
    {
        Ok(ws) => ws,
        Err(e) => {
            if let Some(t) = e.trace() {
                show_trace(t);
            }
            if json {
                crate::emit_json(&crate::json_out::failure(Some(&plan.request), &e));
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
    show_trace(&ws.handshake.trace);
    let ms = ws.handshake.timings.total.as_secs_f64() * 1000.0;
    printer.event(
        "open",
        &match &ws.protocol {
            Some(p) => format!("connected to {} (subprotocol {p}) in {ms:.0} ms", ws.handshake.url),
            None => format!("connected to {} in {ms:.0} ms", ws.handshake.url),
        },
        json!({ "url": printer.hide(&ws.handshake.url), "status": ws.handshake.status, "protocol": ws.protocol }),
    );
    let (mut tx, mut rx) = ws.split();

    for m in plan.messages.iter().cloned() {
        printer.message(true, &m);
        if let Err(e) = tx.send(m).await {
            eprintln!("error: {e}");
            return exit::NETWORK;
        }
    }

    // Lines from stdin are sent when nothing else was given, or with -i.
    let read_stdin = args.interactive || plan.messages.is_empty();
    let (line_tx, mut lines) = tokio::sync::mpsc::channel::<String>(16);
    if read_stdin {
        if std::io::stdin().is_terminal() && !json {
            eprintln!("type a message and press Enter to send it; Ctrl-D closes");
        }
        tokio::spawn(async move {
            let mut input = tokio::io::BufReader::new(tokio::io::stdin()).lines();
            while let Ok(Some(line)) = input.next_line().await {
                if line_tx.send(line).await.is_err() {
                    break;
                }
            }
        });
    } else {
        drop(line_tx);
    }
    let mut input_open = read_stdin;
    let wait = (args.wait > 0.0).then(|| Duration::from_secs_f64(args.wait));
    let mut received = 0usize;
    let mut last = Instant::now();
    let interrupt = tokio::signal::ctrl_c();
    tokio::pin!(interrupt);

    loop {
        // Once everything is sent, the connection stays open for `--wait` without messages.
        let idle = match (input_open, wait) {
            (false, Some(w)) => w.saturating_sub(last.elapsed()),
            _ => Duration::from_secs(24 * 3600),
        };
        tokio::select! {
            m = rx.recv() => match m {
                Some(Ok(m)) => {
                    printer.message(false, &m);
                    last = Instant::now();
                    if let WsMessage::Close { code, reason } = &m {
                        printer.event("close", &format!("closed by the server ({})", close_text(*code, reason)),
                            json!({ "by": "server", "code": code, "reason": printer.hide(reason) }));
                        let _ = tx.close(*code, "").await;
                        break exit::OK;
                    }
                    if matches!(m, WsMessage::Text { .. } | WsMessage::Binary { .. }) {
                        received += 1;
                        if args.count.is_some_and(|n| received >= n) {
                            let _ = tx.close(None, "").await;
                            printer.event("close", "closed after the requested number of messages", json!({ "by": "client", "code": 1000 }));
                            break exit::OK;
                        }
                    }
                }
                Some(Err(e)) => {
                    printer.event("error", &format!("error: {e}"), json!({ "message": printer.hide(&e.to_string()) }));
                    break exit::NETWORK;
                }
                None => {
                    printer.event("close", "the connection was closed", json!({ "by": "server" }));
                    break exit::OK;
                }
            },
            line = lines.recv(), if input_open => match line {
                Some(text) => {
                    let m = WsMessage::Text { data: text };
                    printer.message(true, &m);
                    if let Err(e) = tx.send(m).await {
                        eprintln!("error: {e}");
                        break exit::NETWORK;
                    }
                    last = Instant::now();
                }
                None => {
                    input_open = false;
                    last = Instant::now();
                }
            },
            () = tokio::time::sleep(idle), if !input_open && wait.is_some() => {
                let _ = tx.close(None, "").await;
                printer.event("close", "closed (nothing more arrived)", json!({ "by": "client", "code": 1000 }));
                break exit::OK;
            }
            _ = &mut interrupt => {
                let _ = tx.close(Some(1001), "").await;
                printer.event("close", "closed (interrupted)", json!({ "by": "client", "code": 1001 }));
                break exit::OK;
            }
        }
    }
}

fn close_text(code: Option<u16>, reason: &str) -> String {
    match (code, reason.is_empty()) {
        (Some(c), true) => c.to_string(),
        (Some(c), false) => format!("{c} {reason}"),
        (None, _) => "no status".into(),
    }
}
