//! `/api/contexts`: the sessions and subagents this server has seen, and the
//! text one of them would be given next.
//!
//! A context key contains slashes (`machine/session-id`, and `agent-id` after it
//! for a subagent), so one wildcard route takes everything under
//! `/api/contexts/` and the trailing segment selects the sub-resource, the way
//! `/api/memories/` does.

use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use serde::Deserialize;

use crate::app::AppState;
use crate::context::ContextKey;
use crate::operations::{self, PromptMode};

use super::{Rejection, answer, resource};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/contexts", get(list))
        .route("/contexts/{*rest}", get(read))
}

/// `GET /api/contexts`
async fn list(State(state): State<Arc<AppState>>) -> Response {
    resource(operations::contexts(&state.contexts, state.clock.now()).await)
}

/// Which text of a context the query asks for.
///
/// An absent mode is what the next event would deliver, which is what the key
/// alone is read as asking about.
#[derive(Debug, Deserialize)]
struct PromptQuery {
    mode: Option<String>,
}

impl PromptQuery {
    fn mode(&self) -> Result<PromptMode, Rejection> {
        match self.mode.as_deref() {
            None | Some("") | Some("due") => Ok(PromptMode::Due),
            Some("all") => Ok(PromptMode::All),
            Some(other) => Err(Rejection::bad_request(format!(
                "mode must be `due` or `all`, not `{other}`"
            ))),
        }
    }
}

/// `GET /api/contexts/{*key}/prompt`: the text the context would be given,
/// rendered without recording anything against it.
async fn read(
    State(state): State<Arc<AppState>>,
    Path(rest): Path<String>,
    Query(query): Query<PromptQuery>,
) -> Response {
    let ContextRoute::Prompt(key) = ContextRoute::of(&rest) else {
        return unknown_path(&rest);
    };
    let key = match ContextKey::parse(&key) {
        Ok(key) => key,
        Err(error) => return Rejection::bad_request(error.to_string()).into_response(),
    };
    let mode = match query.mode() {
        Ok(mode) => mode,
        Err(rejection) => return rejection.into_response(),
    };
    answer(operations::context_prompt(&state, &key, mode).await)
}

/// What the path after `/api/contexts/` names.
#[derive(Debug, PartialEq, Eq)]
enum ContextRoute {
    /// The text one context would be given, by the context's key.
    Prompt(String),
    /// Nothing this API answers: no key, or no action after it.
    Unknown,
}

impl ContextRoute {
    fn of(rest: &str) -> Self {
        let segments: Vec<&str> = rest.split('/').filter(|part| !part.is_empty()).collect();
        match segments.as_slice() {
            [.., "prompt"] if segments.len() >= 2 => {
                ContextRoute::Prompt(segments[..segments.len() - 1].join("/"))
            }
            _ => ContextRoute::Unknown,
        }
    }
}

/// A path under `/api/contexts/` that names no resource of this API.
fn unknown_path(rest: &str) -> Response {
    Rejection::not_found(format!(
        "/api/contexts/{rest} is not a path this API answers"
    ))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Detects a split that takes the trailing `prompt` segment as part of the
    /// key, which would make every prompt a context nobody has been seen at, and
    /// one that drops a subagent's agent id from the key.
    #[test]
    fn the_trailing_segment_selects_the_action_and_the_rest_is_the_context_key() {
        assert_eq!(
            ContextRoute::of("alpha/session-1/prompt"),
            ContextRoute::Prompt("alpha/session-1".to_string())
        );
        assert_eq!(
            ContextRoute::of("alpha/session-1/agent-7/prompt"),
            ContextRoute::Prompt("alpha/session-1/agent-7".to_string())
        );
        assert_eq!(ContextRoute::of("alpha/session-1"), ContextRoute::Unknown);
        assert_eq!(ContextRoute::of("prompt"), ContextRoute::Unknown);
        assert_eq!(ContextRoute::of(""), ContextRoute::Unknown);
    }
}
