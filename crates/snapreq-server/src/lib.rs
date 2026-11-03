//! SnapRequest self-hosted server: REST and WebSocket API, storage and authentication.
//!
//! [`serve`] runs the HTTP server for a [`Config`]; [`app`] is the router on its own (for
//! tests and embedding).

pub mod config;

use std::future::Future;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

pub use config::{Config, Overrides};

/// Errors produced by `snapreq-server`.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An error from the core HTTP engine.
    #[error(transparent)]
    Core(#[from] snapreq_core::Error),
    /// The configuration is invalid.
    #[error("{0}")]
    Config(String),
    /// Starting or running the server failed.
    #[error("{context}: {source}")]
    Io {
        context: String,
        #[source]
        source: std::io::Error,
    },
}

/// Convenience alias for results in `snapreq-server`.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Shared state of the running server.
#[derive(Debug)]
pub struct AppState {
    pub config: Config,
}

/// The HTTP routes.
pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/api/v1/version", get(version))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

async fn health() -> Json<Value> {
    Json(json!({ "status": "ok" }))
}

async fn version(State(_state): State<Arc<AppState>>) -> Json<Value> {
    Json(json!({
        "name": "snapreq",
        "version": snapreq_core::VERSION,
        "api": 1,
    }))
}

/// Completes on Ctrl-C, or on SIGTERM on Unix (how containers are stopped).
pub async fn shutdown_signal() {
    let ctrl_c = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut s) => {
                s.recv().await;
            }
            Err(_) => std::future::pending::<()>().await,
        }
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }
    tracing::info!("shutting down");
}

/// A server that is listening but not yet serving; lets the caller learn the real
/// address (when the port was `0`) before running it.
#[derive(Debug)]
pub struct Server {
    listener: TcpListener,
    state: Arc<AppState>,
}

impl Server {
    /// Creates the data directory and binds the listener.
    pub async fn bind(config: Config) -> Result<Self> {
        std::fs::create_dir_all(&config.data_dir).map_err(|source| Error::Io {
            context: format!(
                "cannot create the data directory `{}`",
                config.data_dir.display()
            ),
            source,
        })?;
        let listener = TcpListener::bind(config.bind)
            .await
            .map_err(|source| Error::Io {
                context: format!("cannot listen on {}", config.bind),
                source,
            })?;
        if !config.bind.ip().is_loopback() {
            tracing::warn!(
                bind = %config.bind,
                "listening on a non-loopback address without authentication"
            );
        }
        Ok(Self {
            listener,
            state: Arc::new(AppState { config }),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr> {
        self.listener.local_addr().map_err(|source| Error::Io {
            context: "cannot read the listening address".into(),
            source,
        })
    }

    /// Serves until `shutdown` completes, then finishes the requests in flight.
    pub async fn run(self, shutdown: impl Future<Output = ()> + Send + 'static) -> Result<()> {
        axum::serve(self.listener, app(self.state))
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(|source| Error::Io {
                context: "server failed".into(),
                source,
            })
    }
}
