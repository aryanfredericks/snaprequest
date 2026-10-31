//! `snapreq`: fast, self-hosted HTTP API testing and debugging.

mod cli;
mod collection_cmd;
#[cfg(test)]
mod fixtures;
mod json_out;
mod output;
mod send;
#[cfg(test)]
mod snapshot_tests;
mod trace_view;
mod waterfall;

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Parser;
use snapreq_core::{Client, Error};
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, ColorArg, Command, FormatArg, NewCommand, SecretCommand, SendArgs};
use crate::output::Parts;

/// Process exit codes. Documented in `docs/CLI.md`; keep both in sync.
pub(crate) mod exit {
    /// A response was received (any status, unless `--fail`).
    pub const OK: u8 = 0;
    /// Unexpected internal error.
    pub const INTERNAL: u8 = 1;
    // 2 is used by the argument parser for usage errors.
    /// The request is invalid (bad URL, unreadable file, bad option value).
    pub const INVALID_REQUEST: u8 = 3;
    /// `--fail` and the response status was 400 or higher.
    pub const HTTP_ERROR: u8 = 4;
    /// DNS, connection, TLS or protocol failure.
    pub const NETWORK: u8 = 5;
    /// A connect or total timeout fired.
    pub const TIMEOUT: u8 = 6;
}

fn init_logging(level: &str) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_env("SNAPREQ_LOG").or_else(|_| EnvFilter::try_new(level))?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    Ok(())
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Err(e) = init_logging(&cli.log_level) {
        eprintln!("error: invalid --log-level: {e}");
        return ExitCode::from(exit::INVALID_REQUEST);
    }
    tracing::debug!(core = snapreq_core::VERSION, "snapreq started");

    let code = match cli.command {
        Command::Send(args) => run_send(&args).await,
        Command::Ls(args) => report(collection_cmd::ls(
            args.dir.as_deref(),
            &mut std::io::stdout().lock(),
        )),
        Command::New(NewCommand::Collection { dir, name }) => report(
            collection_cmd::new_collection(&dir, name.as_deref())
                .map(|p| eprintln!("created {}", p.display())),
        ),
        Command::New(NewCommand::Request { path, target, name }) => report(
            collection_cmd::new_request(&path, &target, name.as_deref())
                .map(|p| eprintln!("created {}", p.display())),
        ),
        Command::Secret(SecretCommand::Set {
            name,
            env,
            collection,
        }) => report(
            collection_cmd::secret_set(&name, env.as_deref(), collection.as_deref())
                .map(|p| eprintln!("stored `{name}` in {}", p.display())),
        ),
    };
    ExitCode::from(code)
}

async fn run_send(args: &SendArgs) -> u8 {
    let json = args.format == FormatArg::Json;
    let color = match args.color {
        ColorArg::Auto => anstream::ColorChoice::Auto,
        ColorArg::Always => anstream::ColorChoice::Always,
        ColorArg::Never => anstream::ColorChoice::Never,
    };
    // Reports a problem found before anything was sent.
    let invalid = |message: String| {
        if json {
            emit_json(&json_out::invalid_arguments(&message));
        } else {
            eprintln!("error: {message}");
        }
        exit::INVALID_REQUEST
    };

    let built = collection_cmd::resolve_target(args).and_then(|target| match target {
        collection_cmd::Target::Url => send::build_request(args),
        collection_cmd::Target::Collection { root, key } => {
            collection_cmd::build(&root, &key, args)
        }
    });
    let request = match built {
        Ok(r) => r,
        Err(e) => return invalid(e.to_string()),
    };
    let terminal = std::io::stdout().is_terminal();
    let mut parts = match &args.print {
        Some(spec) => match Parts::parse(spec) {
            Ok(p) => p,
            Err(e) => return invalid(e),
        },
        None => Parts::default_for(terminal),
    };
    if let Some(path) = &args.output {
        // Check before sending, so a request is not made only to lose its result.
        if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty())
            && !dir.is_dir()
        {
            return invalid(format!(
                "cannot save to `{}`: directory `{}` does not exist",
                path.display(),
                dir.display()
            ));
        }
        parts.response_body = false;
    }

    let show_trace = |trace: &[snapreq_core::model::TraceEntry]| {
        if args.verbose > 0 {
            let mut err = anstream::AutoStream::new(std::io::stderr().lock(), color);
            // Losing the debug view is not worth failing the command over.
            let _ = trace_view::render(&mut err, trace, args.verbose);
            let _ = std::io::Write::write_all(&mut err, b"\n");
            let _ = waterfall::render(
                &mut err,
                trace,
                waterfall::terminal_width(waterfall::Stream::Stderr),
            );
        }
    };

    match Client::default().execute(&request).await {
        Ok(response) => {
            show_trace(&response.trace);
            if let Some(path) = &args.output {
                if let Err(e) = std::fs::write(path, &response.body) {
                    eprintln!("error: cannot write `{}`: {e}", path.display());
                    return exit::INTERNAL;
                }
                if !json && std::io::stderr().is_terminal() {
                    eprintln!(
                        "saved {} to {}",
                        output::human_bytes(response.body.len() as u64),
                        path.display()
                    );
                }
            }
            if json {
                emit_json(&json_out::success(
                    &request,
                    &response,
                    args.output.as_deref(),
                ));
            } else {
                let mut out = anstream::AutoStream::new(std::io::stdout().lock(), color);
                if let Err(e) = output::print(&mut out, &request, &response, parts, terminal) {
                    // A closed pipe (`| head`) is not an error worth reporting.
                    if e.kind() != std::io::ErrorKind::BrokenPipe {
                        eprintln!("error: {e}");
                        return exit::INTERNAL;
                    }
                }
            }
            if args.fail && response.status >= 400 {
                exit::HTTP_ERROR
            } else {
                exit::OK
            }
        }
        Err(e) => {
            if let Some(trace) = e.trace() {
                show_trace(trace);
            }
            if json {
                emit_json(&json_out::failure(Some(&request), &e));
            } else {
                let hint = if args.verbose == 0 && e.trace().is_some() {
                    " (run with -v to see what happened)"
                } else {
                    ""
                };
                eprintln!("error: {e}{hint}");
            }
            match e {
                Error::Execution(ex) if ex.timed_out => exit::TIMEOUT,
                Error::Execution(_) => exit::NETWORK,
                _ => exit::INVALID_REQUEST,
            }
        }
    }
}

/// Prints an error for commands other than `send`; returns the exit code.
fn report(result: Result<(), send::ArgError>) -> u8 {
    match result {
        Ok(()) => exit::OK,
        Err(e) => {
            eprintln!("error: {e}");
            exit::INVALID_REQUEST
        }
    }
}

/// Writes one JSON document to stdout.
fn emit_json(doc: &serde_json::Value) {
    let mut out = std::io::stdout().lock();
    // Serializing a `Value` cannot fail; a closed pipe is not worth reporting.
    let _ = serde_json::to_writer_pretty(&mut out, doc);
    let _ = std::io::Write::write_all(&mut out, b"\n");
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        Cli::command().debug_assert();
    }
}
