//! `GET /api/stats/*`: the aggregates of the statistics log.
//!
//! Every aggregate but two is a query over the sqlite log. The exceptions are
//! the live-context count and the catalog's memory count on the scopes report:
//! how many contexts are working in a scope right now, and how many memories a
//! scope holds today, are not things an append-only log can answer.
//!
//! Characters become tokens here, through `stats::tokens_of` and the store's
//! `characters_per_token`, so that the page never converts anything itself.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

use crate::app::AppState;
use crate::context::ContextKey;
use crate::stats::{
    Bucket, Filter, MemoryStatsRow, ScopeStatsRow, SessionStatsRow, StatsError, StatsReader,
    SummaryRow, Window, tokens_of,
};
use crate::store::ScopeId;

use super::Rejection;

/// The statistics routes, as paths under `/api`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/stats/memories", get(memories))
        .route("/stats/triggers", get(triggers))
        .route("/stats/scopes", get(scopes))
        .route("/stats/denies", get(denies))
        .route("/stats/latency", get(latency))
        .route("/stats/sessions", get(sessions))
        .route("/stats/summary", get(summary))
        .route("/stats/series", get(series))
        .route("/stats/session/{*rest}", get(session_series))
}

/// The window and the filters every report takes, as query parameters.
///
/// A parameter that is absent or empty narrows nothing, so a request with no
/// query at all reads the whole log.
#[derive(Debug, Default, Deserialize)]
struct FilterQuery {
    from: Option<String>,
    to: Option<String>,
    session: Option<String>,
    scope: Option<String>,
}

impl FilterQuery {
    /// The filter this query asks for, or what about it could not be read.
    fn filter(&self) -> Result<Filter, Rejection> {
        Ok(Filter {
            window: Window {
                from: instant("from", self.from.as_deref())?,
                to: instant("to", self.to.as_deref())?,
            },
            session: match self.session.as_deref().filter(|key| !key.is_empty()) {
                None => None,
                Some(key) => Some(
                    ContextKey::parse(key)
                        .map_err(|error| Rejection::bad_request(error.to_string()))?,
                ),
            },
            scope: self
                .scope
                .as_deref()
                .filter(|scope| !scope.is_empty())
                .map(str::to_string),
        })
    }
}

/// One bound of the window, refused with the name of the parameter that could
/// not be read so that a caller knows which of the two to fix.
fn instant(name: &str, text: Option<&str>) -> Result<Option<DateTime<Utc>>, Rejection> {
    let Some(text) = text.filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    DateTime::parse_from_rfc3339(text)
        .map(|instant| Some(instant.with_timezone(&Utc)))
        .map_err(|error| Rejection::bad_request(format!("{name} is not an RFC 3339 time: {error}")))
}

/// What each memory was shown for, with index lines and full bodies apart, and
/// what it cost.
async fn memories(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FilterQuery>,
) -> Response {
    let filter = match query.filter() {
        Ok(filter) => filter,
        Err(rejection) => return rejection.into_response(),
    };
    let Some(divisor) = characters_per_token(&state).await else {
        return store_unreadable();
    };
    let rows = read(&state, move |reader| reader.memory_stats(&filter)).await;
    answer(rows.map(|rows| {
        rows.into_iter()
            .map(|row| MemoryTokensRow {
                tokens: tokens_of(row.chars, divisor),
                row,
            })
            .collect::<Vec<_>>()
    }))
}

/// What each trigger pattern did.
async fn triggers(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FilterQuery>,
) -> Response {
    let filter = match query.filter() {
        Ok(filter) => filter,
        Err(rejection) => return rejection.into_response(),
    };
    answer(read(&state, move |reader| reader.trigger_stats(&filter)).await)
}

/// What each scope did, what its sections cost, how many memories it holds and
/// how many live contexts work in it.
async fn scopes(State(state): State<Arc<AppState>>, Query(query): Query<FilterQuery>) -> Response {
    let filter = match query.filter() {
        Ok(filter) => filter,
        Err(rejection) => return rejection.into_response(),
    };
    let active_sets = active_scope_sets(&state).await;
    let catalog = match state.store.snapshot().await {
        Ok(catalog) => catalog,
        Err(error) => {
            tracing::error!("the store could not be read: {error}");
            return store_unreadable();
        }
    };
    let divisor = catalog.settings().characters_per_token;
    // The memories a scope holds now, which is the catalog's answer and not the
    // log's: a memory moved out of a scope stops being one of its memories at
    // once, however many times it was delivered under it.
    let mut held: BTreeMap<String, u64> = BTreeMap::new();
    for memory in catalog.memories() {
        for scope in memory.scopes() {
            *held.entry(scope.to_string()).or_default() += 1;
        }
    }
    let rows = read(&state, move |reader| {
        reader.scope_stats(&filter, &active_sets)
    })
    .await;
    answer(rows.map(|rows| {
        rows.into_iter()
            .map(|row| {
                let tokens = tokens_of(row.chars, divisor);
                ScopeTokensRow {
                    tokens,
                    // A scope that delivered nothing has no cost per delivery.
                    tokens_per_delivery: match row.deliveries {
                        0 => 0.0,
                        deliveries => tokens as f64 / deliveries as f64,
                    },
                    memories: held.get(&row.scope_id).copied().unwrap_or(0),
                    row,
                }
            })
            .collect::<Vec<_>>()
    }))
}

/// Stopped tool calls per day, against every event of that day.
async fn denies(State(state): State<Arc<AppState>>) -> Response {
    answer(read(&state, |reader| reader.deny_days()).await)
}

/// How long the server took to answer each kind of event.
async fn latency(State(state): State<Arc<AppState>>) -> Response {
    answer(read(&state, |reader| reader.latency()).await)
}

/// What each session was delivered, and the context size its last event
/// reported.
async fn sessions(
    State(state): State<Arc<AppState>>,
    Query(query): Query<FilterQuery>,
) -> Response {
    let filter = match query.filter() {
        Ok(filter) => filter,
        Err(rejection) => return rejection.into_response(),
    };
    let Some(divisor) = characters_per_token(&state).await else {
        return store_unreadable();
    };
    let rows = read(&state, move |reader| reader.session_stats(&filter)).await;
    answer(rows.map(|rows| {
        rows.into_iter()
            .map(|row| SessionTokensRow {
                tokens: tokens_of(row.chars, divisor),
                row,
            })
            .collect::<Vec<_>>()
    }))
}

/// What was delivered, answered, held and forgotten over the last five minutes,
/// hour, day and week, with the contexts live now.
async fn summary(State(state): State<Arc<AppState>>) -> Response {
    let Some(divisor) = characters_per_token(&state).await else {
        return store_unreadable();
    };
    let live_contexts = active_scope_sets(&state).await.len() as u64;
    let now = state.clock.now();
    let rows = read(&state, move |reader| reader.summary(now)).await;
    answer(rows.map(|rows| {
        Summary {
            windows: rows
                .into_iter()
                .map(|row| SummaryWindow {
                    tokens: tokens_of(row.chars, divisor),
                    row,
                })
                .collect(),
            live_contexts,
        }
    }))
}

/// Delivered tokens per bucket over the window, for one session or one scope
/// when the query names one.
async fn series(State(state): State<Arc<AppState>>, Query(query): Query<SeriesQuery>) -> Response {
    let filter = match query.filter.filter() {
        Ok(filter) => filter,
        Err(rejection) => return rejection.into_response(),
    };
    let Some(divisor) = characters_per_token(&state).await else {
        return store_unreadable();
    };
    let asked = query.bucket.as_deref().unwrap_or("auto");
    let chosen = match asked {
        "" | "auto" => None,
        name => match Bucket::parse(name) {
            Some(bucket) => Some(bucket),
            None => {
                return Rejection::bad_request(format!(
                    "bucket must be `auto`, `minute`, `hour` or `day`, not `{name}`"
                ))
                .into_response();
            }
        },
    };
    let now = state.clock.now();
    let answered = read(&state, move |reader| {
        let bucket = match chosen {
            Some(bucket) => bucket,
            None => Bucket::automatic(automatic_span(reader, &filter, now)?),
        };
        Ok(Series {
            bucket: bucket.as_str().to_string(),
            points: reader
                .series(&filter, bucket)?
                .into_iter()
                .map(|point| SeriesPoint {
                    t: point.t,
                    tokens: tokens_of(point.chars, divisor),
                    deliveries: point.deliveries,
                })
                .collect(),
        })
    })
    .await;
    answer(answered)
}

/// The span `auto` chooses its bucket from: the window the request named, or
/// the span of the events it selects when it named none.
fn automatic_span(
    reader: &StatsReader,
    filter: &Filter,
    now: DateTime<Utc>,
) -> Result<Duration, StatsError> {
    let recorded = reader.span(filter)?;
    let parse = |text: &str| {
        DateTime::parse_from_rfc3339(text)
            .ok()
            .map(|instant| instant.with_timezone(&Utc))
    };
    let from = filter
        .window
        .from
        .or_else(|| recorded.as_ref().and_then(|(first, _)| parse(first)));
    let to = filter
        .window
        .to
        .or_else(|| recorded.as_ref().and_then(|(_, last)| parse(last)))
        .unwrap_or(now);
    Ok(match from {
        Some(from) => to - from,
        None => Duration::zero(),
    })
}

/// `GET /api/stats/session/{*key}/series`: every hook event of one context with
/// the context size it reported and the tokens its answer carried.
async fn session_series(
    State(state): State<Arc<AppState>>,
    Path(rest): Path<String>,
    Query(query): Query<FilterQuery>,
) -> Response {
    let Some(key) = series_key(&rest) else {
        return Rejection::not_found(format!(
            "/api/stats/session/{rest} is not a path this API answers"
        ))
        .into_response();
    };
    let key = match ContextKey::parse(&key) {
        Ok(key) => key,
        Err(error) => return Rejection::bad_request(error.to_string()).into_response(),
    };
    let window = match query.filter() {
        Ok(filter) => filter.window,
        Err(rejection) => return rejection.into_response(),
    };
    let Some(divisor) = characters_per_token(&state).await else {
        return store_unreadable();
    };
    let named = key.to_string();
    let rows = read(&state, move |reader| reader.session_series(&key, window)).await;
    let rows = match rows {
        Ok(rows) => rows,
        Err(error) => return unreadable(error),
    };
    // A context the log has no event of is a resource that is not there, the
    // same answer a memory that does not exist gets.
    if rows.is_empty() {
        return Rejection::not_found(format!("the log has no events of {named}")).into_response();
    }
    super::resource(
        rows.into_iter()
            .map(|row| SessionEventPoint {
                t: row.t,
                event: row.event,
                context_tokens: row.context_tokens,
                tokens: tokens_of(row.answer_chars, divisor),
            })
            .collect::<Vec<_>>(),
    )
}

/// The context key in a path under `/api/stats/session/`, whose last segment
/// selects the sub-resource the way `/api/contexts/` does.
fn series_key(rest: &str) -> Option<String> {
    let segments: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    match segments.as_slice() {
        [.., "series"] if segments.len() >= 3 => Some(segments[..segments.len() - 1].join("/")),
        _ => None,
    }
}

/// What a request for a series asks for, which is a filter and a bucket.
#[derive(Debug, Deserialize)]
struct SeriesQuery {
    #[serde(flatten)]
    filter: FilterQuery,
    bucket: Option<String>,
}

/// The summary: one row per window, with the contexts live now.
#[derive(Serialize)]
struct Summary {
    windows: Vec<SummaryWindow>,
    live_contexts: u64,
}

#[derive(Serialize)]
struct SummaryWindow {
    #[serde(flatten)]
    row: SummaryRow,
    tokens: u64,
}

#[derive(Serialize)]
struct Series {
    /// The bucket the points are in, which is the one `auto` chose when the
    /// request asked for one.
    bucket: String,
    points: Vec<SeriesPoint>,
}

#[derive(Serialize)]
struct SeriesPoint {
    t: String,
    tokens: u64,
    deliveries: u64,
}

#[derive(Serialize)]
struct SessionEventPoint {
    t: String,
    event: String,
    context_tokens: Option<u64>,
    tokens: u64,
}

#[derive(Serialize)]
struct MemoryTokensRow {
    #[serde(flatten)]
    row: MemoryStatsRow,
    tokens: u64,
}

#[derive(Serialize)]
struct ScopeTokensRow {
    #[serde(flatten)]
    row: ScopeStatsRow,
    tokens: u64,
    /// The tokens one delivery of this scope's section cost on average.
    tokens_per_delivery: f64,
    /// Memories the catalog gives this scope now.
    memories: u64,
}

#[derive(Serialize)]
struct SessionTokensRow {
    #[serde(flatten)]
    row: SessionStatsRow,
    tokens: u64,
}

/// The divisor every token figure of this answer goes through, read from the
/// store the server is serving, or `None` when the store cannot be read, which
/// the caller answers with [`store_unreadable`].
async fn characters_per_token(state: &Arc<AppState>) -> Option<f64> {
    match state.store.snapshot().await {
        Ok(catalog) => Some(catalog.settings().characters_per_token),
        Err(error) => {
            tracing::error!("the store could not be read: {error}");
            None
        }
    }
}

/// What a caller is told when the store the token figures are divided by cannot
/// be read: the setting is the store's, so a store that is unreadable is a
/// server fault like an unreadable log.
fn store_unreadable() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(serde_json::json!({ "error": "the store could not be read" })),
    )
        .into_response()
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

/// Run one query against this server's own statistics log.
pub(crate) async fn read<T: Send + 'static>(
    state: &AppState,
    query: impl FnOnce(&StatsReader) -> Result<T, StatsError> + Send + 'static,
) -> Result<T, StatsError> {
    crate::stats::read(&state.stats, &state.config.stats_path, query).await
}

/// The rows as JSON, or a 500 in the shape every other failure of the API takes.
fn answer<T: Serialize>(result: Result<T, StatsError>) -> Response {
    match result {
        Ok(rows) => super::resource(rows),
        Err(error) => unreadable(error),
    }
}

/// What a caller is told when the log itself cannot be read: a log that is not
/// there is a server fault, not a request the client got wrong, so the detail
/// goes to the server's log and the answer says only that.
pub(crate) fn unreadable(error: StatsError) -> Response {
    tracing::error!("the statistics could not be read: {error}");
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        axum::Json(serde_json::json!({ "error": "the statistics could not be read" })),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a split that takes the trailing `series` segment as part of the
    /// key, which would make every request a context nobody has been seen at,
    /// and one that drops a subagent's agent id from the key.
    #[test]
    fn the_trailing_segment_selects_the_series_and_the_rest_is_the_context_key() {
        assert_eq!(
            series_key("alpha/session-1/series"),
            Some("alpha/session-1".to_string())
        );
        assert_eq!(
            series_key("alpha/session-1/agent-7/series"),
            Some("alpha/session-1/agent-7".to_string())
        );
        assert_eq!(series_key("alpha/session-1"), None);
        assert_eq!(series_key("series"), None);
        assert_eq!(series_key(""), None);
    }
}
