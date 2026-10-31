//! `snapreq`: fast, self-hosted HTTP API testing and debugging.

mod cli;
mod output;
mod send;

use std::io::IsTerminal;
use std::process::ExitCode;

use clap::Parser;
use snapreq_core::{Client, Error};
use tracing_subscriber::EnvFilter;

use crate::cli::{Cli, ColorArg, Command, SendArgs};
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
    };
    ExitCode::from(code)
}

async fn run_send(args: &SendArgs) -> u8 {
    let request = match send::build_request(args) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("error: {e}");
            return exit::INVALID_REQUEST;
        }
    };
    let terminal = std::io::stdout().is_terminal();
    let parts = match &args.print {
        Some(spec) => match Parts::parse(spec) {
            Ok(p) => p,
            Err(e) => {
                eprintln!("error: {e}");
                return exit::INVALID_REQUEST;
            }
        },
        None => Parts::default_for(terminal),
    };
    let color = match args.color {
        ColorArg::Auto => anstream::ColorChoice::Auto,
        ColorArg::Always => anstream::ColorChoice::Always,
        ColorArg::Never => anstream::ColorChoice::Never,
    };

    match Client::default().execute(&request).await {
        Ok(response) => {
            let mut out = anstream::AutoStream::new(std::io::stdout().lock(), color);
            if let Err(e) = output::print(&mut out, &request, &response, parts, terminal) {
                // A closed pipe (`| head`) is not an error worth reporting.
                if e.kind() != std::io::ErrorKind::BrokenPipe {
                    eprintln!("error: {e}");
                    return exit::INTERNAL;
                }
            }
            if args.fail && response.status >= 400 {
                exit::HTTP_ERROR
            } else {
                exit::OK
            }
        }
        Err(e) => {
            eprintln!("error: {e}");
            match e {
                Error::Execution(ex) if ex.timed_out => exit::TIMEOUT,
                Error::Execution(_) => exit::NETWORK,
                _ => exit::INVALID_REQUEST,
            }
        }
    }
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
