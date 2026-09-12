//! The history routes, which answer the same way for a memory and for a scope.
//!
//! They are not a router of their own: a memory's history hangs off the wildcard
//! route in [`super::memories`] because a memory id contains slashes, while a
//! scope's is an ordinary path. Both end here, so the two resources cannot start
//! reporting history differently.

use crate::app::AppState;
use crate::operations::{self, DocumentKind};
use axum::response::Response;

use super::answer;

/// The commits that changed one document, newest first.
pub async fn commits(state: &AppState, kind: DocumentKind, id: &str) -> Response {
    answer(operations::history(state, kind, id).await)
}

/// One document at one commit, with the change that commit made.
pub async fn entry(state: &AppState, kind: DocumentKind, id: &str, oid: &str) -> Response {
    answer(operations::history_entry(state, kind, id, oid).await)
}
