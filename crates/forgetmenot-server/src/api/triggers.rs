//! `POST /api/triggers/test`: which scopes a text would turn on.
//!
//! A test only reports; it never changes a context, so a person can try a
//! pattern against a real store without affecting any live session.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::post;

use crate::app::AppState;
use crate::operations::{self, OperationError, TriggerTestRequest};

use super::{parse_body, resource};

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/triggers/test", post(test))
}

async fn test(State(state): State<Arc<AppState>>, body: Bytes) -> Response {
    let request: TriggerTestRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    let catalog = match state.store.snapshot().await {
        Ok(catalog) => catalog,
        Err(error) => return OperationError::from(error).into_response(),
    };
    resource(operations::trigger_test(&catalog, &request))
}
