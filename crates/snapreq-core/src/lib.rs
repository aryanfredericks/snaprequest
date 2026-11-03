//! SnapRequest core: the HTTP engine, request/response model and trace events.
//!
//! This crate must stay free of UI, CLI and storage concerns so that the CLI,
//! the server and any future front end can all share it.

mod auth;
pub mod client;
pub mod error;
pub mod model;
mod serde_util;

pub use client::{Certificate, Client, ClientConfig, DEFAULT_SENSITIVE_HEADERS, StoredCookie};
pub use error::{Error, ExecutionError, Result};
pub use model::{Body, Request, Response};

/// Version of the SnapRequest core crate.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
