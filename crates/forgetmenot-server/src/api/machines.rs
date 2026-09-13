//! `GET /api/machines`: every machine name this server knows.
//!
//! The frontend offers these wherever a machine is picked: the machine a trigger
//! test pretends to run on, and the machine a `working_directory` or `any`
//! trigger is qualified for. A name is worth offering as soon as the server has
//! ever heard from the machine, so the list is the contexts it holds together
//! with the machines the statistics log recorded.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;

use crate::app::AppState;
use crate::operations;

use super::resource;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/machines", get(list))
}

async fn list(State(state): State<Arc<AppState>>) -> Response {
    let recorded = match super::stats::read(&state, |reader| reader.machines()).await {
        Ok(machines) => machines,
        Err(error) => return super::stats::unreadable(error),
    };
    resource(operations::machines(&state.contexts, state.clock.now(), recorded).await)
}
