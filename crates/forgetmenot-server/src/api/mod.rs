//! The JSON API the frontend uses.
//!
//! Every handler is a thin adapter: it reads the request, calls one function in
//! [`crate::operations`] and turns the answer into a status and a JSON body.
//! Nothing about memories, scopes or sessions is decided here, so the API and
//! the MCP tools cannot drift apart.
//!
//! The status codes are the contract the frontend is built against: 200 with the
//! resource, 409 carrying the document as the store has it now, 422 carrying one
//! message per problem, 404 and 400 carrying `{"error": ...}`.

pub mod contexts;
pub mod history;
pub mod memories;
pub mod review;
pub mod scopes;
pub mod stats;
pub mod triggers;

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::app::AppState;
use crate::operations::OperationError;

/// Every route under `/api`.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .merge(memories::router())
        .merge(scopes::router())
        .merge(triggers::router())
        .merge(contexts::router())
        .merge(review::router())
        .merge(stats::router())
}

/// A request this server refuses before any operation runs: a path that names
/// nothing, or a body or query it cannot read.
#[derive(Clone, Debug)]
pub struct Rejection {
    status: StatusCode,
    message: String,
}

impl Rejection {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }
}

impl IntoResponse for Rejection {
    fn into_response(self) -> Response {
        error_response(self.status, &self.message)
    }
}

/// A resource, as `200` and its JSON.
pub fn resource<Body: Serialize>(body: Body) -> Response {
    Json(body).into_response()
}

/// An operation's answer: the resource it produced, or the status and body its
/// failure maps to.
pub fn answer<Body: Serialize>(outcome: Result<Body, OperationError>) -> Response {
    match outcome {
        Ok(body) => resource(body),
        Err(error) => error.into_response(),
    }
}

/// Parse a request body, refusing it with what was wrong when it is not the
/// shape the route takes.
///
/// Done here rather than with `Json<T>` as an extractor so that a malformed body
/// is reported as JSON like every other failure, which is what the frontend's
/// client reads.
pub fn parse_body<Body: DeserializeOwned>(bytes: &Bytes) -> Result<Body, Rejection> {
    serde_json::from_slice(bytes)
        .map_err(|error| Rejection::bad_request(format!("the body is not valid: {error}")))
}

/// What the caller is told about a failed operation.
///
/// The two failures the frontend acts on carry a body it can use: the current
/// document on a conflict, the list of problems on a refusal. Everything else is
/// a message.
impl IntoResponse for OperationError {
    fn into_response(self) -> Response {
        match self {
            OperationError::NotFound(_) => error_response(StatusCode::NOT_FOUND, &self.to_string()),
            OperationError::Conflict { ref current, .. } => (
                StatusCode::CONFLICT,
                Json(serde_json::json!({ "current": current })),
            )
                .into_response(),
            OperationError::Invalid { ref errors } => (
                StatusCode::UNPROCESSABLE_ENTITY,
                Json(serde_json::json!({ "errors": errors })),
            )
                .into_response(),
            OperationError::Busy => {
                error_response(StatusCode::SERVICE_UNAVAILABLE, &self.to_string())
            }
            OperationError::UnknownScope(_)
            | OperationError::ImplicitScope(_)
            | OperationError::UnknownSession(_) => {
                error_response(StatusCode::BAD_REQUEST, &self.to_string())
            }
            OperationError::Store(_) | OperationError::Git(_) | OperationError::Render(_) => {
                // The caller cannot act on these, so the detail goes to the log
                // as well as into the answer.
                tracing::error!("an API request failed: {self}");
                error_response(StatusCode::INTERNAL_SERVER_ERROR, &self.to_string())
            }
        }
    }
}

fn error_response(status: StatusCode, message: &str) -> Response {
    (status, Json(serde_json::json!({ "error": message }))).into_response()
}
