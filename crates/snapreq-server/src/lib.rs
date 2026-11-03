//! SnapRequest self-hosted server: REST and WebSocket API, storage and authentication.
//!
//! [`serve`] runs the HTTP server for a [`Config`]; [`app`] is the router on its own (for
//! tests and embedding).

pub mod api;
pub mod config;
pub mod store;
pub mod ws;

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
    /// The database failed.
    #[error("storage: {0}")]
    Storage(String),
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
    pub store: store::Store,
    /// Sends the requests the API executes (shared connection pool and cookie jar).
    pub client: snapreq_core::Client,
}

/// How often history is pruned while the server runs.
const PRUNE_EVERY: std::time::Duration = std::time::Duration::from_secs(3600);

/// The HTTP routes.
pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/healthz", get(health))
        .route("/api/v1/version", get(version))
        .route("/api/v1/ws", get(ws::upgrade))
        .nest("/api/v1", api::routes())
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
        std::fs::create_dir_all(&config.workspace).map_err(|source| Error::Io {
            context: format!(
                "cannot create the workspace `{}`",
                config.workspace.display()
            ),
            source,
        })?;
        let store = store::Store::open(&config.data_dir.join(store::DB_FILE)).await?;
        let pruned = store.prune(config.retention).await?;
        if pruned != store::Pruned::default() {
            tracing::info!(
                history = pruned.history,
                runs = pruned.runs,
                "pruned old entries"
            );
        }
        if !config.bind.ip().is_loopback() {
            tracing::warn!(
                bind = %config.bind,
                "listening on a non-loopback address without authentication"
            );
        }
        Ok(Self {
            listener,
            state: Arc::new(AppState {
                config,
                store,
                client: snapreq_core::Client::default(),
            }),
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
        let (store, retention) = (self.state.store.clone(), self.state.config.retention);
        let pruner = tokio::spawn(async move {
            let mut every = tokio::time::interval(PRUNE_EVERY);
            every.tick().await; // the first tick is immediate; pruning already ran in `bind`
            loop {
                every.tick().await;
                if let Err(e) = store.prune(retention).await {
                    tracing::warn!("pruning history failed: {e}");
                }
            }
        });
        let result = axum::serve(self.listener, app(self.state))
            .with_graceful_shutdown(shutdown)
            .await
            .map_err(|source| Error::Io {
                context: "server failed".into(),
                source,
            });
        pruner.abort();
        result
    }

    /// The server's database.
    pub fn store(&self) -> &store::Store {
        &self.state.store
    }
}
