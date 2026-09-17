//! The history routes: the store's own commits, and the commits of one document.
//!
//! A memory's history hangs off the wildcard route in [`super::memories`]
//! because a memory id contains slashes, while a scope's is an ordinary path.
//! Both end in [`commits`] and [`entry`], so the two resources cannot start
//! reporting history differently. The store's commit list and one commit's files
//! are routes of their own, under `/history`.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;

use crate::app::AppState;
use crate::operations::{self, DEFAULT_HISTORY_LIMIT, DocumentKind};

use super::{Rejection, answer};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/history", get(store_commits))
        .route("/history/{oid}", get(store_commit))
}

/// The page of the store's history a request asks for.
///
/// `limit` arrives as text and is checked here, so a mistyped number is a `400`
/// naming what was expected rather than a page of some other size.
#[derive(Debug, Deserialize)]
struct PageQuery {
    before: Option<String>,
    limit: Option<String>,
}

impl PageQuery {
    /// The commit the page starts after, and how many commits it holds.
    fn page(self) -> Result<(Option<String>, usize), Rejection> {
        let limit = match self.limit.as_deref().filter(|text| !text.is_empty()) {
            None => DEFAULT_HISTORY_LIMIT,
            Some(text) => text.parse().map_err(|_| {
                Rejection::bad_request(format!("limit must be a whole number, not `{text}`"))
            })?,
        };
        Ok((self.before.filter(|oid| !oid.is_empty()), limit))
    }
}

/// `GET /api/history`: the store's commits, newest first.
async fn store_commits(
    State(state): State<Arc<AppState>>,
    Query(query): Query<PageQuery>,
) -> Response {
    let (before, limit) = match query.page() {
        Ok(page) => page,
        Err(rejection) => return rejection.into_response(),
    };
    answer(operations::store_history(&state, before, limit).await)
}

/// `GET /api/history/{oid}`: one commit with every file it changed.
async fn store_commit(State(state): State<Arc<AppState>>, Path(oid): Path<String>) -> Response {
    answer(operations::store_commit(&state, &oid).await)
}

/// The commits that changed one document, newest first.
pub async fn commits(state: &AppState, kind: DocumentKind, id: &str) -> Response {
    answer(operations::history(state, kind, id).await)
}

/// One document at one commit, with the change that commit made.
pub async fn entry(state: &AppState, kind: DocumentKind, id: &str, oid: &str) -> Response {
    answer(operations::history_entry(state, kind, id, oid).await)
}
