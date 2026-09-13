//! `/api/branches`: open a transaction, list the open ones, see what one holds,
//! land it as one commit, or drop it.
//!
//! A branch name has a `/` in it, so one wildcard route takes everything under
//! `/api/branches/` and the trailing segment selects the sub-resource, the same
//! way the memory routes work.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::app::AppState;
use crate::operations::branches::{self, BranchCreateRequest, LandRequest};
use crate::store::branch::BranchName;

use super::{Rejection, answer, parse_body};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/branches", get(list).post(create))
        .route("/branches/{*rest}", get(read).post(act).delete(remove))
}

/// `GET /api/branches`: every open branch, with its owner and ahead/behind.
async fn list(State(state): State<Arc<AppState>>) -> Response {
    answer(branches::branch_list(&state).await)
}

/// `POST /api/branches`: open a branch for the calling session.
async fn create(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: BranchCreateRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    answer(branches::branch_create(&state, &request.session_key).await)
}

/// `GET /api/branches/{name}/diff`
async fn read(State(state): State<Arc<AppState>>, Path(rest): Path<String>) -> Response {
    let (branch, action) = match split(&rest) {
        Ok(parts) => parts,
        Err(rejection) => return rejection.into_response(),
    };
    match action.as_deref() {
        Some("diff") => answer(branches::branch_diff(&state, &branch).await),
        _ => unknown_path(&rest),
    }
}

/// `POST /api/branches/{name}/land`
async fn act(
    State(state): State<Arc<AppState>>,
    Path(rest): Path<String>,
    body: Bytes,
) -> Response {
    let (branch, action) = match split(&rest) {
        Ok(parts) => parts,
        Err(rejection) => return rejection.into_response(),
    };
    if action.as_deref() != Some("land") {
        return unknown_path(&rest);
    }
    let request: LandRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    answer(branches::branch_land(&state, &branch, &request).await)
}

/// `DELETE /api/branches/{name}`
async fn remove(State(state): State<Arc<AppState>>, Path(rest): Path<String>) -> Response {
    let (branch, action) = match split(&rest) {
        Ok(parts) => parts,
        Err(rejection) => return rejection.into_response(),
    };
    if action.is_some() {
        return unknown_path(&rest);
    }
    answer(branches::branch_abandon(&state, &branch).await)
}

/// The branch and the sub-resource a path under `/api/branches/` names.
///
/// A branch name is two segments, so a third segment is the sub-resource and a
/// fourth is nothing this API answers.
fn split(rest: &str) -> Result<(BranchName, Option<String>), Rejection> {
    let segments: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
    let (name, action) = match segments.as_slice() {
        [prefix, name] => (format!("{prefix}/{name}"), None),
        [prefix, name, action] => (format!("{prefix}/{name}"), Some((*action).to_string())),
        _ => return Err(unknown(rest)),
    };
    let branch =
        BranchName::parse(&name).map_err(|error| Rejection::bad_request(error.to_string()))?;
    Ok((branch, action))
}

fn unknown(rest: &str) -> Rejection {
    Rejection::not_found(format!(
        "/api/branches/{rest} is not a path this API answers"
    ))
}

fn unknown_path(rest: &str) -> Response {
    unknown(rest).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a split that takes the branch's own `/` as a sub-resource, which
    /// would make every diff and every land a 404.
    #[test]
    fn the_branch_is_two_segments_and_the_third_is_the_sub_resource() {
        let (branch, action) = split("tx/alpha-session-1-1").expect("a branch with no action");
        assert_eq!(branch.as_str(), "tx/alpha-session-1-1");
        assert_eq!(action, None);

        let (branch, action) = split("tx/alpha-session-1-1/diff").expect("a branch and an action");
        assert_eq!(branch.as_str(), "tx/alpha-session-1-1");
        assert_eq!(action.as_deref(), Some("diff"));

        assert!(split("tx").is_err(), "one segment names no branch");
        assert!(
            split("main/land").is_err(),
            "a branch outside the prefix is not a transaction"
        );
    }
}
