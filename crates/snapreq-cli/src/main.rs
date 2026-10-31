//! `snapreq`: fast, self-hosted HTTP API testing and debugging.

use clap::Parser;
use tracing_subscriber::EnvFilter;

/// Fast, self-hosted HTTP API testing and debugging.
#[derive(Debug, Parser)]
#[command(name = "snapreq", version, about, long_about = None)]
struct Cli {
    /// Internal log level for SnapRequest itself (error, warn, info, debug, trace).
    /// Overridden by the `SNAPREQ_LOG` environment variable.
    #[arg(long, global = true, default_value = "warn")]
    log_level: String,
}

fn init_logging(level: &str) -> anyhow::Result<()> {
    let filter = EnvFilter::try_from_env("SNAPREQ_LOG").or_else(|_| EnvFilter::try_new(level))?;
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();
    Ok(())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    init_logging(&cli.log_level)?;
    tracing::debug!(core = snapreq_core::VERSION, "snapreq started");
    Ok(())
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
