//! `GET /api/contexts`: the sessions and subagents this server has seen.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;

use crate::app::AppState;
use crate::operations;

use super::resource;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/contexts", get(list))
}

async fn list(State(state): State<Arc<AppState>>) -> Response {
    resource(operations::contexts(&state.contexts, state.clock.now()).await)
}
