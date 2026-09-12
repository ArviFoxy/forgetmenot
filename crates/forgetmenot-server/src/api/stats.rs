//! `GET /api/stats/*`: the aggregates of the statistics log.
//!
//! Every aggregate but one is a query over the sqlite log. The exception is the
//! live-context count on the scopes report, which is the registry's state: how
//! many contexts are working in a scope right now is not something an
//! append-only log can answer.

use std::collections::BTreeSet;
use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Serialize;

use crate::app::AppState;
use crate::stats::{StatsError, StatsReader};
use crate::store::ScopeId;

/// The statistics routes, as paths under `/api`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/stats/memories", get(memories))
        .route("/stats/triggers", get(triggers))
        .route("/stats/scopes", get(scopes))
        .route("/stats/denies", get(denies))
        .route("/stats/latency", get(latency))
        .route("/stats/sessions", get(sessions))
}

/// What each memory was shown for, with index lines and full bodies apart.
async fn memories(State(state): State<Arc<AppState>>) -> Response {
    answer(read(&state, |reader| reader.memory_stats()).await)
}

/// What each trigger pattern did.
async fn triggers(State(state): State<Arc<AppState>>) -> Response {
    answer(read(&state, |reader| reader.trigger_stats()).await)
}

/// What each scope did, and how many live contexts work in it.
async fn scopes(State(state): State<Arc<AppState>>) -> Response {
    let active_sets = active_scope_sets(&state).await;
    answer(read(&state, move |reader| reader.scope_stats(&active_sets)).await)
}

/// Stopped tool calls per day, against every event of that day.
async fn denies(State(state): State<Arc<AppState>>) -> Response {
    answer(read(&state, |reader| reader.deny_days()).await)
}

/// How long the server took to answer each kind of event.
async fn latency(State(state): State<Arc<AppState>>) -> Response {
    answer(read(&state, |reader| reader.latency()).await)
}

/// Delivered bytes per session, in both forms.
async fn sessions(State(state): State<Arc<AppState>>) -> Response {
    answer(read(&state, |reader| reader.session_bytes()).await)
}

/// The active scope set of every live context, which is what a live-context
/// count is counted over.
async fn active_scope_sets(state: &Arc<AppState>) -> Vec<BTreeSet<ScopeId>> {
    state
        .contexts
        .snapshot(state.clock.now())
        .await
        .contexts
        .into_iter()
        .map(|record| record.state.active)
        .collect()
}

/// Flush what the hot path has queued, then run `query` on a blocking thread.
///
/// The flush is what makes a report cover the requests answered before it: the
/// statistics are written off the hot path, so without it a reader can miss the
/// event it was opened to look at. sqlite is a blocking API, so the query
/// itself does not run on an async worker.
async fn read<T: Send + 'static>(
    state: &Arc<AppState>,
    query: impl FnOnce(&StatsReader) -> Result<T, StatsError> + Send + 'static,
) -> Result<T, StatsError> {
    state.stats.flush().await;
    let path = state.config.stats_path.clone();
    match tokio::task::spawn_blocking({
        let path = path.clone();
        move || query(&StatsReader::open(&path)?)
    })
    .await
    {
        Ok(result) => result,
        Err(error) => Err(StatsError::Sqlite {
            path,
            source: rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(error))),
        }),
    }
}

/// The rows as JSON, or a 500 in the shape every other failure of the API takes:
/// a log that cannot be read is a server fault, not a request the client got
/// wrong, so the detail goes to the log and the answer says only that.
fn answer<T: Serialize>(result: Result<T, StatsError>) -> Response {
    match result {
        Ok(rows) => super::resource(rows),
        Err(error) => {
            tracing::error!("the statistics could not be read: {error}");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                axum::Json(serde_json::json!({ "error": "the statistics could not be read" })),
            )
                .into_response()
        }
    }
}
