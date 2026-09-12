//! `GET /api/review`: everything wrong with the store, and what is worth a
//! second look.

use std::sync::Arc;

use axum::Router;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;

use crate::app::AppState;
use crate::operations::{self, OperationError};

use super::resource;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/review", get(report))
}

async fn report(State(state): State<Arc<AppState>>) -> Response {
    let catalog = match state.store.snapshot().await {
        Ok(catalog) => catalog,
        Err(error) => return OperationError::from(error).into_response(),
    };
    resource(operations::review(&catalog))
}
