//! `POST /api/triggers/test`: which scopes a text would turn on, and
//! `POST /api/triggers/validate`: whether a pattern is a regex at all.
//!
//! A test only reports; it never changes a context, so a person can try a
//! pattern against a real store without affecting any live session. A validate
//! reads nothing at all: it compiles the pattern and says what the compiler
//! said, so an editor can check a pattern on every keystroke.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::post;

use crate::app::AppState;
use crate::operations::{self, OperationError, PatternRequest, TriggerTestRequest};

use super::{parse_body, resource};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/triggers/test", post(test))
        .route("/triggers/validate", post(validate))
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

/// Whether a pattern compiles. A pattern that does not is not a failed request:
/// the answer is what is wrong with it, which is what the editor shows.
async fn validate(body: Bytes) -> Response {
    let request: PatternRequest = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    resource(operations::validate_pattern(&request))
}
