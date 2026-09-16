//! `/api/scopes`: the index, one scope, writes, and a scope's history.
//!
//! A scope id has no slashes, so these are ordinary paths rather than the
//! wildcard the memory routes need.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::app::AppState;
use crate::operations::{
    self, DeleteRequest, DocumentKind, OperationError, ScopeCreateRequest, ScopeWriteRequest,
};
use crate::store::ScopeId;
use crate::store::validate::WriteMode;

use super::{BranchQuery, answer, parse_body};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/scopes", get(index).post(create))
        .route("/scopes/{id}", get(read).put(replace).delete(remove))
        .route("/scopes/{id}/history", get(history))
        .route("/scopes/{id}/history/{oid}", get(history_entry))
}

/// `GET /api/scopes`
async fn index(State(state): State<Arc<AppState>>) -> Response {
    answer(operations::scope_index(&state).await)
}

/// `GET /api/scopes/{id}`
async fn read(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    let catalog = match state.store.snapshot().await {
        Ok(catalog) => catalog,
        Err(error) => return OperationError::from(error).into_response(),
    };
    answer(operations::scope_get(&catalog, &ScopeId::new(id)))
}

/// `POST /api/scopes`: create one scope.
async fn create(
    State(state): State<Arc<AppState>>,
    Query(query): Query<BranchQuery>,
    body: Bytes,
) -> Response {
    let branch = match query.branch() {
        Ok(branch) => branch,
        Err(rejection) => return rejection.into_response(),
    };
    let request: ScopeCreateRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    answer(
        operations::scope_put(
            &state,
            &request.id,
            &request.write,
            WriteMode::Create,
            branch.as_ref(),
        )
        .await,
    )
}

/// `PUT /api/scopes/{id}`: replace the version the caller read.
async fn replace(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<BranchQuery>,
    body: Bytes,
) -> Response {
    let branch = match query.branch() {
        Ok(branch) => branch,
        Err(rejection) => return rejection.into_response(),
    };
    let request: ScopeWriteRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    answer(
        operations::scope_put(
            &state,
            &ScopeId::new(id),
            &request,
            WriteMode::Update,
            branch.as_ref(),
        )
        .await,
    )
}

/// `DELETE /api/scopes/{id}`: remove the version the caller read.
async fn remove(
    State(state): State<Arc<AppState>>,
    Path(id): Path<String>,
    Query(query): Query<BranchQuery>,
    body: Bytes,
) -> Response {
    let branch = match query.branch() {
        Ok(branch) => branch,
        Err(rejection) => return rejection.into_response(),
    };
    let request: DeleteRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    answer(operations::scope_delete(&state, &ScopeId::new(id), &request, branch.as_ref()).await)
}

/// `GET /api/scopes/{id}/history`
async fn history(State(state): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    super::history::commits(&state, DocumentKind::Scope, &id).await
}

/// `GET /api/scopes/{id}/history/{oid}`
async fn history_entry(
    State(state): State<Arc<AppState>>,
    Path((id, oid)): Path<(String, String)>,
) -> Response {
    super::history::entry(&state, DocumentKind::Scope, &id, &oid).await
}
