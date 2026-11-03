//! SnapRequest assertions, collection runner and reporters.

pub mod assert;
pub mod capture;
pub mod data;
pub mod flow;
pub mod run;
pub mod session;

pub use assert::{Assertions, Outcome};
pub use capture::{Captured, Capturer};
pub use flow::{FlowRun, StepRun, run_flow};
pub use run::{Iteration, Plan, Unit, UnitRun, execute, request_keys};
pub use session::{RequestRun, RunOutcome, SendSpec, Session, Target};

/// Errors produced by `snapreq-test`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An error from the core HTTP engine.
    #[error(transparent)]
    Core(#[from] snapreq_core::Error),
}

/// Convenience alias for results in `snapreq-test`.
pub type Result<T, E = Error> = std::result::Result<T, E>;
