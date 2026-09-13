//! `/api/settings`: the store's behaviour settings and writes to them.
//!
//! One key at a time, because that is what an editor changes and what a commit
//! is worth making about: the answer names the version the whole file was read
//! at, and the next write sends it back.

use std::sync::Arc;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use serde::Deserialize;

use crate::app::AppState;
use crate::operations::settings::{self, SettingsWriteRequest};
use crate::store::branch::BranchName;

use super::{Rejection, answer, parse_body};

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/settings", get(read))
        .route("/settings/{key}", put(write))
}

/// A write of one setting.
///
/// The branch is in the body rather than the query string, because a settings
/// write carries no document of its own and everything it says about itself
/// belongs in one place.
#[derive(Debug, Deserialize)]
struct SettingsWriteBody {
    #[serde(flatten)]
    write: SettingsWriteRequest,
    #[serde(default)]
    branch: Option<String>,
}

impl SettingsWriteBody {
    fn branch(&self) -> Result<Option<BranchName>, Rejection> {
        match self.branch.as_deref().filter(|name| !name.is_empty()) {
            None => Ok(None),
            Some(name) => BranchName::parse(name)
                .map(Some)
                .map_err(|error| Rejection::bad_request(error.to_string())),
        }
    }
}

/// `GET /api/settings`
async fn read(State(state): State<Arc<AppState>>) -> Response {
    answer(settings::settings_get(&state, None).await)
}

/// `PUT /api/settings/{key}`: change one setting.
async fn write(
    State(state): State<Arc<AppState>>,
    Path(key): Path<String>,
    body: Bytes,
) -> Response {
    let request: SettingsWriteBody = match parse_body(&body) {
        Ok(request) => request,
        Err(rejection) => return rejection.into_response(),
    };
    let branch = match request.branch() {
        Ok(branch) => branch,
        Err(rejection) => return rejection.into_response(),
    };
    answer(settings::settings_set(&state, &key, &request.write, branch.as_ref()).await)
}
