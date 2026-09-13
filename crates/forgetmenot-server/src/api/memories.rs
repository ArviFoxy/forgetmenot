//! `/api/memories`: the index, one document, writes, and a document's history.
//!
//! A memory id contains slashes (a session memory lives at
//! `sessions/<machine>/<session-id>/<name>`), so one wildcard route takes
//! everything under `/api/memories/` and the trailing segments select the
//! sub-resource. The frontend's router splits the same path the same way.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;

use crate::app::AppState;
use crate::operations::{
    self, DeleteRequest, DocumentKind, MemoryCreateRequest, MemoryFilter, MemoryWriteRequest,
    OperationError,
};
use crate::store::MemoryId;
use crate::store::memory::MemoryKind;
use crate::store::validate::WriteMode;

use super::{Rejection, answer, history, parse_body, resource};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/memories", get(index).post(create))
        .route("/memories/{*rest}", get(read).put(replace).delete(remove))
}

/// The index filter, as the query string carries it.
///
/// Every value arrives as text and is checked here, so a mistyped filter is a
/// `400` naming what was expected rather than a silently empty index.
#[derive(Debug, Deserialize)]
struct IndexQuery {
    scope: Option<String>,
    kind: Option<String>,
}

impl IndexQuery {
    fn filter(self) -> Result<MemoryFilter, Rejection> {
        let kind = match self.kind.as_deref() {
            None | Some("") => None,
            Some("critical") => Some(MemoryKind::Critical),
            Some("knowledge") => Some(MemoryKind::Knowledge),
            Some(other) => {
                return Err(Rejection::bad_request(format!(
                    "kind must be `critical` or `knowledge`, not `{other}`"
                )));
            }
        };
        Ok(MemoryFilter {
            scope: self
                .scope
                .filter(|scope| !scope.is_empty())
                .map(crate::store::ScopeId::new),
            kind,
        })
    }
}

/// `GET /api/memories`
async fn index(State(state): State<Arc<AppState>>, Query(query): Query<IndexQuery>) -> Response {
    let filter = match query.filter() {
        Ok(filter) => filter,
        Err(rejection) => return rejection.into_response(),
    };
    let catalog = match state.store.snapshot().await {
        Ok(catalog) => catalog,
        Err(error) => return OperationError::from(error).into_response(),
    };
    resource(operations::memory_index(&catalog, &filter))
}

/// `POST /api/memories`: create one memory.
async fn create(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: MemoryCreateRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    answer(operations::memory_put(&state, &request.id, &request.write, WriteMode::Create).await)
}

/// `GET /api/memories/{*rest}`: the document, its history, or one commit of it.
async fn read(State(state): State<Arc<AppState>>, Path(rest): Path<String>) -> Response {
    match MemoryRoute::of(&rest) {
        MemoryRoute::Document(id) => {
            // No context key: a read through the API is a person looking, not a
            // model being given the memory, so nothing is recorded as delivered.
            answer(operations::memory_get(&state, &MemoryId::new(id), None).await)
        }
        MemoryRoute::History(id) => history::commits(&state, DocumentKind::Memory, &id).await,
        MemoryRoute::HistoryEntry { id, oid } => {
            history::entry(&state, DocumentKind::Memory, &id, &oid).await
        }
        MemoryRoute::Unknown => unknown_path(&rest),
    }
}

/// `PUT /api/memories/{*id}`: replace the version the caller read.
async fn replace(
    State(state): State<Arc<AppState>>,
    Path(rest): Path<String>,
    body: Bytes,
) -> Response {
    let MemoryRoute::Document(id) = MemoryRoute::of(&rest) else {
        return unknown_path(&rest);
    };
    let request: MemoryWriteRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    answer(operations::memory_put(&state, &MemoryId::new(id), &request, WriteMode::Update).await)
}

/// `DELETE /api/memories/{*id}`: remove the version the caller read.
async fn remove(
    State(state): State<Arc<AppState>>,
    Path(rest): Path<String>,
    body: Bytes,
) -> Response {
    let MemoryRoute::Document(id) = MemoryRoute::of(&rest) else {
        return unknown_path(&rest);
    };
    let request: DeleteRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    answer(operations::memory_delete(&state, &MemoryId::new(id), &request).await)
}

/// What the path after `/api/memories/` names.
#[derive(Debug, PartialEq, Eq)]
enum MemoryRoute {
    Document(String),
    History(String),
    HistoryEntry {
        id: String,
        oid: String,
    },
    /// Nothing this API answers: no id at all.
    Unknown,
}

impl MemoryRoute {
    fn of(rest: &str) -> Self {
        let segments: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
        let join = |count: usize| segments[..count].join("/");
        match segments.as_slice() {
            [] => MemoryRoute::Unknown,
            [.., "history", oid] if segments.len() >= 3 => MemoryRoute::HistoryEntry {
                id: join(segments.len() - 2),
                oid: (*oid).to_string(),
            },
            [.., "history"] if segments.len() >= 2 => {
                MemoryRoute::History(join(segments.len() - 1))
            }
            _ => MemoryRoute::Document(segments.join("/")),
        }
    }
}

/// A path under `/api/memories/` that names no resource of this API.
fn unknown_path(rest: &str) -> Response {
    Rejection::not_found(format!(
        "/api/memories/{rest} is not a path this API answers"
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a split that takes the trailing `history` segment as part of the
    /// id, which would make the history of every memory a 404, and one that
    /// takes a session memory's slashes as sub-resources.
    #[test]
    fn the_trailing_segments_select_the_sub_resource_and_the_rest_is_the_id() {
        assert_eq!(
            MemoryRoute::of("sessions/alpha/session-1/notes"),
            MemoryRoute::Document("sessions/alpha/session-1/notes".to_string())
        );
        assert_eq!(
            MemoryRoute::of("bench-power/history"),
            MemoryRoute::History("bench-power".to_string())
        );
        assert_eq!(
            MemoryRoute::of("sessions/alpha/session-1/notes/history"),
            MemoryRoute::History("sessions/alpha/session-1/notes".to_string())
        );
        assert_eq!(
            MemoryRoute::of("bench-power/history/abc123"),
            MemoryRoute::HistoryEntry {
                id: "bench-power".to_string(),
                oid: "abc123".to_string()
            }
        );
        assert_eq!(MemoryRoute::of(""), MemoryRoute::Unknown);
    }
}
