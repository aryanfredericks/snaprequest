//! SnapRequest core: the HTTP engine, request/response model and trace events.
//!
//! This crate must stay free of UI, CLI and storage concerns so that the CLI,
//! the server and any future front end can all share it.

pub mod error;
pub mod model;
mod serde_util;

pub use error::{Error, Result};
pub use model::{Body, Request, Response};

/// Version of the SnapRequest core crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
