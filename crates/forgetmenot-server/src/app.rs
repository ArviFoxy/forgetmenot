//! The HTTP application: shared state, routes, start and shutdown.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use tokio::net::TcpListener;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tower_http::services::{ServeDir, ServeFile};

use crate::clock::Clock;
use crate::config::Config;
use crate::context::registry::{ContextRegistry, SnapshotError};
use crate::hook;
use crate::service::{Store, StoreError};
use crate::stats::{StatsError, StatsWriter};

/// Everything a request handler needs.
pub struct AppState {
    pub store: Arc<Store>,
    pub contexts: Arc<ContextRegistry>,
    pub stats: StatsWriter,
    pub config: Arc<Config>,
    pub clock: Arc<dyn Clock>,
}

/// What stopped the server from starting.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    State(#[from] SnapshotError),
    #[error(transparent)]
    Stats(#[from] StatsError),
    #[error("binding {address} failed: {source}")]
    Bind {
        address: SocketAddr,
        #[source]
        source: std::io::Error,
    },
    #[error("serving failed: {0}")]
    Serve(#[source] std::io::Error),
}

/// The routes the server answers.
///
/// The frontend's files are a fallback, so an API or hook path is never shadowed
/// by a file of the same name, and any path the frontend routes on its own falls
/// back to `index.html`.
pub fn build_router(state: Arc<AppState>) -> Router {
    let web_dist = state.config.web_dist.clone();
    let router = Router::new()
        .route("/hook", post(hook::handle))
        .route("/api/health", get(health))
        .nest("/api", crate::api::router())
        .with_state(state.clone())
        .nest_service("/mcp", crate::mcp::service(state));
    match web_dist {
        Some(directory) => router.fallback_service(spa_service(directory)),
        None => router,
    }
}

/// The built frontend: files from `directory`, and `index.html` for every path
/// that is not a file, because the frontend routes those itself.
fn spa_service(directory: PathBuf) -> ServeDir<ServeFile> {
    let index = directory.join("index.html");
    ServeDir::new(directory).fallback(ServeFile::new(index))
}

/// `GET /api/health`: that the server is up, and which store revision it is
/// answering from.
async fn health(State(state): State<Arc<AppState>>) -> Response {
    match state.store.snapshot().await {
        Ok(catalog) => axum::Json(serde_json::json!({
            "status": "ok",
            "head": catalog.head.to_string(),
        }))
        .into_response(),
        Err(error) => {
            tracing::error!("the store could not be read: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({ "status": "error" })),
            )
                .into_response()
        }
    }
}

/// A bound, running server.
pub struct RunningServer {
    pub address: SocketAddr,
    pub state: Arc<AppState>,
    cancel: CancellationToken,
    server: JoinHandle<Result<(), std::io::Error>>,
    snapshots: JoinHandle<()>,
}

/// Load the recorded contexts, open the store and the statistics, bind the
/// listener and start serving.
///
/// The store and the state file are read before the listener binds, so the first
/// request cannot arrive before the server can answer it.
pub async fn start(config: Config, clock: Arc<dyn Clock>) -> Result<RunningServer, ServeError> {
    let store = Arc::new(Store::open(&config.store_path).await?);
    let contexts = Arc::new(
        ContextRegistry::load_from(
            &config.state_path,
            config.snapshot_debounce_ms,
            config.context_retention_days,
            clock.now(),
        )
        .await?,
    );
    let stats = StatsWriter::open(&config.stats_path)?;
    let config = Arc::new(config);
    let state = Arc::new(AppState {
        store,
        contexts: contexts.clone(),
        stats,
        config: config.clone(),
        clock: clock.clone(),
    });

    let listener = TcpListener::bind(config.listen)
        .await
        .map_err(|source| ServeError::Bind {
            address: config.listen,
            source,
        })?;
    let address = listener.local_addr().map_err(|source| ServeError::Bind {
        address: config.listen,
        source,
    })?;

    let cancel = CancellationToken::new();
    let snapshots = tokio::spawn(snapshot_loop(
        contexts,
        config.state_path.clone(),
        clock,
        cancel.clone(),
    ));
    let shutdown = cancel.clone();
    let router = build_router(state.clone());
    let server = tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async move { shutdown.cancelled().await })
            .await
    });

    Ok(RunningServer {
        address,
        state,
        cancel,
        server,
        snapshots,
    })
}

impl RunningServer {
    /// Stop serving, then flush the statistics and write the contexts, so that a
    /// restart starts from what the last event left behind.
    pub async fn shutdown(self) -> Result<(), ServeError> {
        self.cancel.cancel();
        match self.server.await {
            Ok(result) => result.map_err(ServeError::Serve)?,
            Err(error) => tracing::error!("the server task did not finish: {error}"),
        }
        if let Err(error) = self.snapshots.await {
            tracing::error!("the snapshot task did not finish: {error}");
        }
        self.state.stats.flush().await;
        self.state
            .contexts
            .snapshot_to(&self.state.config.state_path, self.state.clock.now())
            .await?;
        Ok(())
    }

    /// Serve until the process is asked to stop, then shut down.
    pub async fn run_until_signalled(self) -> Result<(), ServeError> {
        wait_for_stop_signal().await;
        tracing::info!("stopping");
        self.shutdown().await
    }
}

/// Write the contexts whenever they change, no more often than the debounce
/// window allows.
async fn snapshot_loop(
    contexts: Arc<ContextRegistry>,
    path: PathBuf,
    clock: Arc<dyn Clock>,
    cancel: CancellationToken,
) {
    while contexts.wait_for_change(&cancel).await {
        if let Err(error) = contexts.snapshot_to(&path, clock.now()).await {
            tracing::error!("writing the context state failed: {error}");
        }
    }
}

/// Ctrl-C, or SIGTERM from a service manager.
async fn wait_for_stop_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(error) => {
                    tracing::error!("SIGTERM cannot be watched: {error}");
                    let _ = tokio::signal::ctrl_c().await;
                    return;
                }
            };
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}
